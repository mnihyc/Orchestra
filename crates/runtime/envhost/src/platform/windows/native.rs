//! Safe relative opens over the Windows NT file interface.
//!
//! Every child name is one checked component. Pointer storage outlives its synchronous
//! call, no pointers escape, and only a successful new owned handle becomes a File.

use std::{
    ffi::{OsStr, OsString, c_void},
    fs::File,
    io,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle},
    },
    ptr,
};

type Handle = *mut c_void;
#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}
#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root: Handle,
    name: *mut UnicodeString,
    attributes: u32,
    security: *mut c_void,
    security_quality: *mut c_void,
}
#[repr(C)]
struct IoStatus {
    status_or_pointer: usize,
    information: usize,
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtCreateFile(
        file: *mut Handle,
        access: u32,
        object: *mut ObjectAttributes,
        status: *mut IoStatus,
        allocation: *mut i64,
        attributes: u32,
        sharing: u32,
        disposition: u32,
        options: u32,
        ea: *mut c_void,
        ea_length: u32,
    ) -> i32;
    fn NtSetInformationFile(
        file: Handle,
        status: *mut IoStatus,
        information: *mut c_void,
        length: u32,
        class: u32,
    ) -> i32;
    fn NtQueryDirectoryFile(
        file: Handle,
        event: Handle,
        apc: *mut c_void,
        apc_context: *mut c_void,
        status: *mut IoStatus,
        information: *mut c_void,
        length: u32,
        class: u32,
        single: u8,
        pattern: *mut UnicodeString,
        restart: u8,
    ) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn DeviceIoControl(
        file: Handle,
        code: u32,
        input: *mut c_void,
        input_length: u32,
        output: *mut c_void,
        output_length: u32,
        returned: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
}
const SYNCHRONIZE: u32 = 0x0010_0000;
const READ_ATTRIBUTES: u32 = 0x80;
const WRITE_ATTRIBUTES: u32 = 0x100;
const READ_DATA: u32 = 1;
const WRITE_DATA: u32 = 2;
const DELETE: u32 = 0x0001_0000;
const OPEN: u32 = 1;
const CREATE: u32 = 2;
const DIRECTORY: u32 = 1;
const NON_DIRECTORY: u32 = 0x40;
const SYNCHRONOUS: u32 = 0x20;
const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const CASE_INSENSITIVE: u32 = 0x40;
const DONT_REPARSE: u32 = 0x1000;
const SHARE_ALL: u32 = 7;

fn checked_name(name: &OsStr) -> io::Result<Vec<u16>> {
    let name: Vec<u16> = name.encode_wide().collect();
    if name.is_empty()
        || name.as_slice() == [b'.' as u16]
        || name.as_slice() == [b'.' as u16, b'.' as u16]
        || name
            .iter()
            .any(|unit| [0, b'/' as u16, b'\\' as u16, b':' as u16].contains(unit))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a plain name",
        ));
    }
    Ok(name)
}

fn open(
    parent: &File,
    name: &OsStr,
    access: u32,
    disposition: u32,
    options: u32,
) -> io::Result<File> {
    open_name(parent, checked_name(name)?, access, disposition, options)
}

fn open_name(
    parent: &File,
    mut name: Vec<u16>,
    access: u32,
    disposition: u32,
    options: u32,
) -> io::Result<File> {
    let length = name
        .len()
        .checked_mul(2)
        .and_then(|length| u16::try_from(length).ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the native name cannot be represented",
            )
        })?;
    let mut unicode = UnicodeString {
        length,
        maximum_length: length,
        buffer: name.as_mut_ptr(),
    };
    let mut object = ObjectAttributes {
        length: std::mem::size_of::<ObjectAttributes>() as u32,
        root: parent.as_raw_handle(),
        name: &mut unicode,
        attributes: CASE_INSENSITIVE | DONT_REPARSE,
        security: ptr::null_mut(),
        security_quality: ptr::null_mut(),
    };
    let mut status = IoStatus {
        status_or_pointer: 0,
        information: 0,
    };
    let mut handle = ptr::null_mut();
    // SAFETY: all pointers name initialized, correctly aligned storage retained through this
    // synchronous call; the borrowed parent is open and the Unicode name has its exact byte length.
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            access | SYNCHRONIZE | READ_ATTRIBUTES,
            &mut object,
            &mut status,
            ptr::null_mut(),
            0,
            SHARE_ALL,
            disposition,
            options | SYNCHRONOUS | OPEN_REPARSE_POINT,
            ptr::null_mut(),
            0,
        )
    };
    if result < 0 {
        // SAFETY: this pure conversion accepts every NTSTATUS and returns a Win32 error number.
        return Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(result) } as i32,
        ));
    }
    if handle.is_null() || handle as isize == -1 {
        return Err(io::Error::other("the native open returned no file handle"));
    }
    // SAFETY: success transferred one newly owned file handle, and no other owner was constructed.
    Ok(unsafe { File::from_raw_handle(handle) })
}

pub(super) fn open_directory(parent: &File, name: &OsStr) -> io::Result<File> {
    open(parent, name, READ_DATA, OPEN, DIRECTORY)
}
pub(super) fn create_directory(parent: &File, name: &OsStr) -> io::Result<File> {
    open(
        parent,
        name,
        READ_DATA | WRITE_ATTRIBUTES,
        CREATE,
        DIRECTORY,
    )
}
pub(super) fn open_metadata(parent: &File, name: &OsStr) -> io::Result<File> {
    open(parent, name, 0, OPEN, 0)
}
pub(super) fn open_read(parent: &File, name: &OsStr) -> io::Result<File> {
    open(parent, name, READ_DATA, OPEN, NON_DIRECTORY)
}
pub(super) fn create_file(parent: &File, name: &OsStr) -> io::Result<File> {
    open(
        parent,
        name,
        READ_DATA | WRITE_DATA | WRITE_ATTRIBUTES,
        CREATE,
        NON_DIRECTORY,
    )
}

pub(super) fn open_attributes(parent: &File, name: &OsStr) -> io::Result<File> {
    open(parent, name, WRITE_ATTRIBUTES, OPEN, 0)
}
pub(super) fn open_delete(parent: &File, name: &OsStr) -> io::Result<File> {
    open(parent, name, DELETE, OPEN, 0)
}
fn result(status: i32) -> io::Result<()> {
    if status < 0 {
        // SAFETY: every NTSTATUS can be converted to its Win32 error number.
        Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(status) } as i32,
        ))
    } else {
        Ok(())
    }
}
#[repr(C)]
struct RenameInformation {
    replace: u8,
    root: Handle,
    name_bytes: u32,
    name: [u16; 1],
}

pub(super) fn rename(file: &File, parent: &File, name: &OsStr, replace: bool) -> io::Result<()> {
    let name = checked_name(name)?;
    let name_bytes = name
        .len()
        .checked_mul(2)
        .and_then(|length| u32::try_from(length).ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the native rename name cannot be represented",
            )
        })?;
    let offset = std::mem::offset_of!(RenameInformation, name);
    let length = offset
        .checked_add(name_bytes as usize)
        .and_then(|length| u32::try_from(length).ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the native rename cannot be represented",
            )
        })?;
    // u64 storage gives the pointer-bearing structure its required native alignment.
    let mut bytes = vec![0_u64; (length as usize).div_ceil(std::mem::size_of::<u64>())];
    let information = bytes.as_mut_ptr().cast::<RenameInformation>();
    // SAFETY: the zeroed, aligned allocation covers every fixed field and all UTF-16
    // units; each field is initialized by address and the copied name is in bounds.
    unsafe {
        ptr::addr_of_mut!((*information).replace).write(u8::from(replace));
        ptr::addr_of_mut!((*information).root).write(parent.as_raw_handle());
        ptr::addr_of_mut!((*information).name_bytes).write(name_bytes);
        ptr::copy_nonoverlapping(
            name.as_ptr(),
            ptr::addr_of_mut!((*information).name).cast::<u16>(),
            name.len(),
        );
    }
    let mut status = IoStatus {
        status_or_pointer: 0,
        information: 0,
    };
    // SAFETY: both borrowed handles stay open, information is fully initialized, and
    // its exact byte length covers the trailing name for this synchronous call.
    result(unsafe {
        NtSetInformationFile(
            file.as_raw_handle(),
            &mut status,
            information.cast(),
            length,
            10,
        )
    })
}

pub(super) fn delete(file: &File) -> io::Result<()> {
    let mut delete = 1_u8;
    let mut status = IoStatus {
        status_or_pointer: 0,
        information: 0,
    };
    // SAFETY: FileDispositionInformation contains one BOOLEAN; the initialized byte
    // and status storage live through the call and the DELETE-capable handle stays open.
    result(unsafe {
        NtSetInformationFile(
            file.as_raw_handle(),
            &mut status,
            ptr::addr_of_mut!(delete).cast(),
            1,
            13,
        )
    })
}

fn bad_data() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "the native file data is incomplete",
    )
}
fn u32_at(bytes: &[u8], offset: usize) -> io::Result<u32> {
    let end = offset.checked_add(4).ok_or_else(bad_data)?;
    let slice = bytes.get(offset..end).ok_or_else(bad_data)?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}
fn u16_at(bytes: &[u8], offset: usize) -> io::Result<u16> {
    let end = offset.checked_add(2).ok_or_else(bad_data)?;
    let slice = bytes.get(offset..end).ok_or_else(bad_data)?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}
fn wide_at(bytes: &[u8], offset: usize, length: usize) -> io::Result<OsString> {
    if length % 2 != 0 {
        return Err(bad_data());
    }
    let end = offset.checked_add(length).ok_or_else(bad_data)?;
    let bytes = bytes.get(offset..end).ok_or_else(bad_data)?;
    Ok(OsString::from_wide(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    ))
}

pub(super) fn visit_names(
    parent: &File,
    visit: &mut dyn FnMut(&OsStr) -> bool,
) -> io::Result<bool> {
    // A fresh file object gives each concurrent scan its own NT enumeration cursor.
    let directory = open_name(parent, vec![b'.' as u16], READ_DATA, OPEN, DIRECTORY)?;
    let mut buffer = vec![0_u64; 8192];
    let mut restart = true;
    loop {
        crate::error::check_cancelled().map_err(io::Error::other)?;
        let length = buffer
            .len()
            .checked_mul(8)
            .and_then(|length| u32::try_from(length).ok())
            .ok_or_else(bad_data)?;
        let mut status = IoStatus {
            status_or_pointer: 0,
            information: 0,
        };
        // SAFETY: the synchronous directory handle stays open and the initialized aligned
        // allocation has exactly length bytes. No APC, event, pattern or retained pointer is used.
        let code = unsafe {
            NtQueryDirectoryFile(
                directory.as_raw_handle(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut status,
                buffer.as_mut_ptr().cast(),
                length,
                12,
                0,
                ptr::null_mut(),
                u8::from(restart),
            )
        };
        if code == 0x8000_0006_u32 as i32 {
            return Ok(true);
        }
        if code == 0x8000_0005_u32 as i32 || code == 0 && status.information == 0 {
            let next = (length as usize)
                .checked_mul(2)
                .filter(|length| *length <= u32::MAX as usize)
                .ok_or_else(bad_data)?;
            buffer.resize(next.div_ceil(8), 0);
            continue;
        }
        result(code)?;
        if status.information > length as usize {
            return Err(bad_data());
        }
        // SAFETY: every byte of the allocation was initialized, the returned length was
        // checked against its full size, and the slice cannot outlive the borrowed buffer.
        let bytes =
            unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), status.information) };
        let mut offset = 0;
        loop {
            crate::error::check_cancelled().map_err(io::Error::other)?;
            let remaining = bytes.get(offset..).ok_or_else(bad_data)?;
            let next = u32_at(remaining, 0)? as usize;
            let entry = if next == 0 {
                remaining
            } else {
                if next < 12 || next % 4 != 0 {
                    return Err(bad_data());
                }
                remaining.get(..next).ok_or_else(bad_data)?
            };
            let name_length = u32_at(entry, 8)? as usize;
            let name = wide_at(entry, 12, name_length)?;
            if name != OsStr::new(".") && name != OsStr::new("..") && !visit(&name) {
                return Ok(false);
            }
            if next == 0 {
                break;
            }
            offset = offset
                .checked_add(next)
                .filter(|offset| *offset < bytes.len())
                .ok_or_else(bad_data)?;
        }
        restart = false;
    }
}

pub(super) fn read_link(file: &File) -> io::Result<OsString> {
    // The maximum reparse data is the Windows control protocol's 16 KiB representation.
    let mut bytes = vec![0_u8; 16384];
    let mut returned = 0;
    // SAFETY: the synchronous opened reparse handle and initialized output buffer stay live
    // through the call; the control has no input or overlapped completion pointer.
    let ok = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            0x0009_00a8,
            ptr::null_mut(),
            0,
            bytes.as_mut_ptr().cast(),
            bytes.len() as u32,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let returned = usize::try_from(returned)
        .ok()
        .filter(|size| *size <= bytes.len())
        .ok_or_else(bad_data)?;
    bytes.truncate(returned);
    let data_length = u16_at(&bytes, 4)? as usize;
    let data_end = data_length
        .checked_add(8)
        .filter(|length| *length <= bytes.len())
        .ok_or_else(bad_data)?;
    bytes.truncate(data_end);
    let tag = u32_at(&bytes, 0)?;
    let start = match tag {
        0xa000_000c => 20,
        0xa000_0003 => 16,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "this reparse kind has no filesystem link target",
            ));
        }
    };
    let offset = u16_at(&bytes, 8)? as usize;
    let length = u16_at(&bytes, 10)? as usize;
    let path = bytes.get(start..).ok_or_else(bad_data)?;
    let name = wide_at(path, offset, length)?;
    let wide: Vec<_> = name.encode_wide().collect();
    if wide.starts_with(&[b'\\' as u16, b'?' as u16, b'?' as u16, b'\\' as u16]) {
        let mut normalized = vec![b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
        normalized.extend_from_slice(&wide[4..]);
        Ok(OsString::from_wide(&normalized))
    } else {
        Ok(name)
    }
}

#[cfg(test)]
pub(super) fn test_set_junction(parent: &File, target: &std::path::Path) -> io::Result<()> {
    let file = open_name(parent, vec![b'.' as u16], WRITE_ATTRIBUTES, OPEN, DIRECTORY)?;
    let print: Vec<_> = target.as_os_str().encode_wide().collect();
    let mut substitute = vec![b'\\' as u16, b'?' as u16, b'?' as u16, b'\\' as u16];
    if print.starts_with(&[b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16]) {
        substitute.extend_from_slice(&print[4..]);
    } else {
        substitute.extend_from_slice(&print);
    }
    let sub_length = u16::try_from(substitute.len() * 2).map_err(|_| bad_data())?;
    let print_length = u16::try_from(print.len() * 2).map_err(|_| bad_data())?;
    let mut path = Vec::new();
    for unit in substitute {
        path.extend_from_slice(&unit.to_le_bytes());
    }
    path.extend_from_slice(&0_u16.to_le_bytes());
    let print_offset = u16::try_from(path.len()).map_err(|_| bad_data())?;
    for unit in print {
        path.extend_from_slice(&unit.to_le_bytes());
    }
    path.extend_from_slice(&0_u16.to_le_bytes());
    let data_length = u16::try_from(8 + path.len()).map_err(|_| bad_data())?;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&0xa000_0003_u32.to_le_bytes());
    bytes.extend_from_slice(&data_length.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    for value in [0, sub_length, print_offset, print_length] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend(path);
    let mut returned = 0;
    // SAFETY: the writable directory handle and initialized protocol buffer outlive this
    // synchronous control; the test supplies one absolute target and no output/overlapped pointer.
    let ok = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            0x0009_00a4,
            bytes.as_mut_ptr().cast(),
            u32::try_from(bytes.len()).map_err(|_| bad_data())?,
            ptr::null_mut(),
            0,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
