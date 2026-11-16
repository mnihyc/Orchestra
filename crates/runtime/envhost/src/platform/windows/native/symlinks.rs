//! A symbolic link is created and tagged on its original handle, with a scoped token copy.

use super::{
    CREATE, DELETE, DIRECTORY, DeviceIoControl, Handle, NON_DIRECTORY, WRITE_ATTRIBUTES,
    checked_name, open_shared,
};
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io,
    marker::PhantomData,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Component, Path, PathBuf},
    ptr,
    rc::Rc,
};

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Luid {
    low: u32,
    high: i32,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Privilege {
    luid: Luid,
    attributes: u32,
}
#[repr(C)]
struct Privileges {
    count: u32,
    privilege: Privilege,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentThread() -> Handle;
    fn GetCurrentProcess() -> Handle;
    fn GetFinalPathNameByHandleW(file: Handle, path: *mut u16, length: u32, flags: u32) -> u32;
}
#[link(name = "advapi32")]
unsafe extern "system" {
    fn OpenThreadToken(thread: Handle, access: u32, as_self: i32, token: *mut Handle) -> i32;
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
    fn DuplicateTokenEx(
        token: Handle,
        access: u32,
        attributes: *mut std::ffi::c_void,
        level: u32,
        kind: u32,
        duplicate: *mut Handle,
    ) -> i32;
    fn LookupPrivilegeValueW(system: *const u16, name: *const u16, luid: *mut Luid) -> i32;
    fn AdjustTokenPrivileges(
        token: Handle,
        disable_all: i32,
        new: *const Privileges,
        length: u32,
        previous: *mut Privileges,
        returned: *mut u32,
    ) -> i32;
    fn SetThreadToken(thread: *mut Handle, token: Handle) -> i32;
    #[cfg(test)]
    fn CreateRestrictedToken(
        token: Handle,
        flags: u32,
        disabled_count: u32,
        disabled: *mut std::ffi::c_void,
        deleted_count: u32,
        deleted: *const Privilege,
        restricted_count: u32,
        restricted: *mut std::ffi::c_void,
        result: *mut Handle,
    ) -> i32;
}

const TOKEN_DUPLICATE: u32 = 2;
const TOKEN_IMPERSONATE: u32 = 4;
const TOKEN_QUERY: u32 = 8;
const TOKEN_ADJUST_PRIVILEGES: u32 = 0x20;

fn owned(ok: i32, handle: Handle) -> io::Result<OwnedHandle> {
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    if handle.is_null() || handle as isize == -1 {
        return Err(io::Error::other("the native token call returned no handle"));
    }
    // SAFETY: the successful token call transferred exactly one new handle to this owner.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn effective_token() -> io::Result<(OwnedHandle, bool)> {
    let mut handle = ptr::null_mut();
    let access = TOKEN_DUPLICATE | TOKEN_IMPERSONATE | TOKEN_QUERY;
    // SAFETY: the thread pseudo-handle is borrowed and only a new token handle is returned.
    let ok = unsafe { OpenThreadToken(GetCurrentThread(), access, 0, &mut handle) };
    if ok != 0 {
        return owned(ok, handle).map(|handle| (handle, true));
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(1008) {
        return Err(error);
    }
    // SAFETY: only a thread with no token falls back to its own process token. No identity is
    // substituted for an existing restricted thread, and the output is one new owned handle.
    let ok = unsafe { OpenProcessToken(GetCurrentProcess(), access, &mut handle) };
    owned(ok, handle).map(|handle| (handle, false))
}

fn duplicate(token: &OwnedHandle) -> io::Result<OwnedHandle> {
    let mut handle = ptr::null_mut();
    // SAFETY: the input stays live; the output is a non-inheritable impersonation copy of the
    // same effective identity. SecurityImpersonation and TokenImpersonation are both enum value 2.
    let ok = unsafe {
        DuplicateTokenEx(
            token.as_raw_handle(),
            TOKEN_DUPLICATE | TOKEN_IMPERSONATE | TOKEN_QUERY | TOKEN_ADJUST_PRIVILEGES,
            ptr::null_mut(),
            2,
            2,
            &mut handle,
        )
    };
    owned(ok, handle)
}

fn symlink_privilege() -> io::Result<Luid> {
    let name: Vec<_> = "SeCreateSymbolicLinkPrivilege\0".encode_utf16().collect();
    let mut luid = Luid::default();
    // SAFETY: the local privilege name is NUL terminated and the output is initialized storage.
    if unsafe { LookupPrivilegeValueW(ptr::null(), name.as_ptr(), &mut luid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(luid)
}

struct TokenScope {
    previous: Option<OwnedHandle>,
    _token: OwnedHandle,
    // The scope cannot be sent to a different thread or retained across an async handoff.
    _thread: PhantomData<Rc<()>>,
}
impl TokenScope {
    fn install(token: OwnedHandle, previous: Option<OwnedHandle>) -> io::Result<Self> {
        // SAFETY: this synchronous scope owns both the installed and previous tokens. A null
        // thread pointer changes only this thread, after successful impersonation is checked.
        if unsafe { SetThreadToken(ptr::null_mut(), token.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            previous,
            _token: token,
            _thread: PhantomData,
        })
    }
    fn symlink() -> io::Result<Self> {
        let (current, was_thread) = effective_token()?;
        let token = duplicate(&current)?;
        let privileges = Privileges {
            count: 1,
            privilege: Privilege {
                luid: symlink_privilege()?,
                attributes: 2,
            },
        };
        // SAFETY: only the private token copy changes, with one exact privilege. No existing
        // process or thread token is adjusted. A missing privilege is left for the filesystem
        // to decide, so Developer Mode remains usable without granting a new privilege.
        if unsafe {
            AdjustTokenPrivileges(
                token.as_raw_handle(),
                0,
                &privileges,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Self::install(token, was_thread.then_some(current))
    }
}
impl Drop for TokenScope {
    fn drop(&mut self) {
        let previous = self
            .previous
            .as_ref()
            .map_or(ptr::null_mut(), AsRawHandle::as_raw_handle);
        // SAFETY: the original token is still owned, or null restores the original process
        // identity. The non-Send scope is dropped on the same thread, including on unwind.
        if unsafe { SetThreadToken(ptr::null_mut(), previous) } == 0 {
            // Continuing on a worker with an unrestored security context would be unsafe.
            std::process::abort();
        }
    }
}

fn opened_path(file: &File) -> io::Result<PathBuf> {
    let mut path = vec![0_u16; 256];
    loop {
        let length = u32::try_from(path.len()).map_err(|_| super::bad_data())?;
        // SAFETY: the handle and initialized UTF16 output allocation remain live throughout.
        let size = unsafe {
            GetFinalPathNameByHandleW(file.as_raw_handle(), path.as_mut_ptr(), length, 0)
        };
        if size == 0 {
            return Err(io::Error::last_os_error());
        }
        if size < length {
            path.truncate(size as usize);
            return Ok(PathBuf::from(OsString::from_wide(&path)));
        }
        path.resize(
            (size as usize).checked_add(1).ok_or_else(super::bad_data)?,
            0,
        );
    }
}

fn target_text(target: &OsStr) -> io::Result<(Vec<u16>, bool)> {
    if target.encode_wide().any(|unit| unit == 0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the link target contains NUL",
        ));
    }
    let path = Path::new(target);
    let absolute =
        path.has_root() || matches!(path.components().next(), Some(Component::Prefix(_)));
    if !absolute {
        return Ok((target.encode_wide().collect(), true));
    }
    let absolute: Vec<_> = std::path::absolute(path)?
        .as_os_str()
        .encode_wide()
        .collect();
    let mut native: Vec<_> = "\\??\\".encode_utf16().collect();
    if absolute.starts_with(&"\\\\?\\".encode_utf16().collect::<Vec<_>>()) {
        native.extend_from_slice(&absolute[4..]);
    } else if absolute.starts_with(&[b'\\' as u16, b'\\' as u16]) {
        native.extend("UNC\\".encode_utf16());
        native.extend_from_slice(&absolute[2..]);
    } else {
        native.extend(absolute);
    }
    Ok((native, false))
}

pub(in super::super) fn create_link(parent: &File, target: &OsStr, name: &OsStr) -> io::Result<()> {
    let name = checked_name(name)?;
    let (target_text, relative) = target_text(target)?;
    let target_length = target_text
        .len()
        .checked_mul(2)
        .ok_or_else(super::bad_data)?;
    let length = 20_usize
        .checked_add(target_length)
        .filter(|size| *size <= 16384)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the native link target cannot be represented",
            )
        })?;
    let target_length = u16::try_from(target_length).map_err(|_| super::bad_data())?;
    let mut data = Vec::with_capacity(length);
    data.extend_from_slice(&0xa000_000c_u32.to_le_bytes());
    data.extend_from_slice(&((length - 8) as u16).to_le_bytes());
    data.extend_from_slice(&0_u16.to_le_bytes());
    for value in [0, target_length, 0, target_length] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    data.extend_from_slice(&u32::from(relative).to_le_bytes());
    for unit in target_text {
        data.extend_from_slice(&unit.to_le_bytes());
    }
    // Resolving the target is only the same directory/file hint the Win32 operation needs.
    // The created leaf is always relative to the held parent, independent of this path lookup.
    let directory = if Path::new(target).is_absolute() {
        std::fs::metadata(target).is_ok_and(|meta| meta.is_dir())
    } else {
        opened_path(parent)
            .is_ok_and(|path| std::fs::metadata(path.join(target)).is_ok_and(|meta| meta.is_dir()))
    };
    let _scope = TokenScope::symlink().ok();
    let file = open_shared(
        parent,
        name,
        WRITE_ATTRIBUTES | DELETE,
        CREATE,
        0x4000 | if directory { DIRECTORY } else { NON_DIRECTORY },
        0,
    )?;
    let mut returned = 0;
    // SAFETY: the exclusive newly created handle and initialized exact reparse buffer remain
    // live until the synchronous control returns. No pointer or partial file is handed out.
    if unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            0x0009_00a4,
            data.as_mut_ptr().cast(),
            length as u32,
            ptr::null_mut(),
            0,
            &mut returned,
            ptr::null_mut(),
        )
    } == 0
    {
        let error = io::Error::last_os_error();
        super::delete(&file).map_err(|cleanup| {
            io::Error::other(format!("{error}; removing the incomplete link: {cleanup}"))
        })?;
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
pub(in super::super) fn test_without_symlink_privilege(test: impl FnOnce()) -> io::Result<()> {
    let (current, was_thread) = effective_token()?;
    let token = duplicate(&current)?;
    let deleted = Privilege {
        luid: symlink_privilege()?,
        attributes: 0,
    };
    let mut restricted = ptr::null_mut();
    // SAFETY: the test removes one named privilege from a copy of its own effective identity;
    // all other arrays are empty, and a successful new handle has exactly one owner.
    let ok = unsafe {
        CreateRestrictedToken(
            token.as_raw_handle(),
            0,
            0,
            ptr::null_mut(),
            1,
            &deleted,
            0,
            ptr::null_mut(),
            &mut restricted,
        )
    };
    let restricted = owned(ok, restricted)?;
    let _scope = TokenScope::install(restricted, was_thread.then_some(current))?;
    test();
    Ok(())
}
