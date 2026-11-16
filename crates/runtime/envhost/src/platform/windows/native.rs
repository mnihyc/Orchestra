//! Safe relative opens over the Windows NT file interface.
//!
//! Every input name is one checked component. Pointer storage outlives its synchronous
//! call, no pointers escape, and only a successful new owned handle becomes a File.

use std::{
    ffi::{OsStr, c_void},
    fs::File,
    io,
    os::windows::{
        ffi::OsStrExt,
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
    fn RtlNtStatusToDosError(status: i32) -> u32;
}
const SYNCHRONIZE: u32 = 0x0010_0000;
const READ_ATTRIBUTES: u32 = 0x80;
const WRITE_ATTRIBUTES: u32 = 0x100;
const READ_DATA: u32 = 1;
const WRITE_DATA: u32 = 2;
const OPEN: u32 = 1;
const CREATE: u32 = 2;
const DIRECTORY: u32 = 1;
const NON_DIRECTORY: u32 = 0x40;
const SYNCHRONOUS: u32 = 0x20;
const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const CASE_INSENSITIVE: u32 = 0x40;
const SHARE_ALL: u32 = 7;

fn open(
    parent: &File,
    name: &OsStr,
    access: u32,
    disposition: u32,
    options: u32,
) -> io::Result<File> {
    let mut name: Vec<u16> = name.encode_wide().collect();
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
        attributes: CASE_INSENSITIVE,
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
