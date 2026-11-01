use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

use boxt_core::compile_db::extension_class;
use boxt_core::fact::FileKind;
use boxt_core::limits::LimitName;
use boxt_core::message::Job;
use boxt_core::outcome::{Details, FileOutcome, IoKind, LinkTarget, NotCheckedReason};
use boxt_core::read::{NotRead, ReadCtx};

use crate::walk::{is_reparse_point, special_kind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByName {
    Listed,
    Extensionless,
    Other,
}

pub fn by_name(file_name: &[u8]) -> ByName {
    if extension_class(file_name).is_some() {
        ByName::Listed
    } else if file_name.iter().skip(1).all(|&b| b != b'.') {
        ByName::Extensionless
    } else {
        ByName::Other
    }
}

pub fn file_kind_by_name(job: &Job) -> Option<FileKind> {
    let path = job.path()?;
    (by_name(path.file_name()) == ByName::Listed).then_some(FileKind::Source)
}

pub fn wants_definitions(file_name: &[u8]) -> bool {
    by_name(file_name) != ByName::Other
}

#[derive(Debug)]
pub enum Fetched {
    Bytes(Vec<u8>),
    NotChecked(FileOutcome, Option<FileKind>),
}

fn not_checked(reason: NotCheckedReason) -> FileOutcome {
    FileOutcome::NotChecked {
        reason,
        details: Details::None,
    }
}

fn after_open_error(path: &Path, error: &io::Error, by_name: Option<FileKind>) -> Fetched {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Fetched::NotChecked(
            not_checked(NotCheckedReason::Symlink(LinkTarget::Unresolved)),
            None,
        ),
        Ok(meta) if is_reparse_point(&meta) || !(meta.is_file() || meta.is_dir()) => {
            Fetched::NotChecked(
                not_checked(NotCheckedReason::SpecialFile(special_kind(&meta))),
                None,
            )
        }
        _ => Fetched::NotChecked(
            not_checked(NotCheckedReason::Unreadable(IoKind::from_io(error.kind()))),
            by_name,
        ),
    }
}

#[cfg(unix)]
fn open_no_follow(path: &Path) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags, open};
    let fd = open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    Ok(File::from(fd))
}

#[cfg(windows)]
fn open_no_follow(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    };
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}

pub fn fetch(path: &Path, max_size: u64, by_name: Option<FileKind>) -> Fetched {
    let file = match open_no_follow(path) {
        Ok(file) => file,
        Err(e) => return after_open_error(path, &e, by_name),
    };
    let meta = match file.metadata() {
        Ok(meta) => meta,
        Err(e) => {
            return Fetched::NotChecked(
                not_checked(NotCheckedReason::Unreadable(IoKind::from_io(e.kind()))),
                by_name,
            );
        }
    };
    if is_reparse_point(&meta) || !(meta.is_file() || meta.is_dir()) {
        return Fetched::NotChecked(
            not_checked(NotCheckedReason::SpecialFile(special_kind(&meta))),
            None,
        );
    }
    if meta.is_dir() {
        return Fetched::NotChecked(
            not_checked(NotCheckedReason::Unreadable(IoKind::IsADirectory)),
            None,
        );
    }
    let size = meta.len();
    if size > max_size {
        return Fetched::NotChecked(
            FileOutcome::NotChecked {
                reason: NotCheckedReason::LimitExceeded(LimitName::FileSize),
                details: Details::Measured {
                    value: size,
                    limit: max_size,
                },
            },
            by_name,
        );
    }
    let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
    match file.take(size).read_to_end(&mut bytes) {
        Ok(_) => Fetched::Bytes(bytes),
        Err(e) => Fetched::NotChecked(
            not_checked(NotCheckedReason::Unreadable(IoKind::from_io(e.kind()))),
            by_name,
        ),
    }
}

#[derive(Debug)]
pub enum Routed {
    Cxx,
    NoReader,
    NotRead(NotRead, Option<FileKind>),
}

pub fn route_content(bytes: &[u8], ctx: &mut ReadCtx<'_>, name: ByName) -> Routed {
    let listed = match name {
        ByName::Listed => true,
        ByName::Extensionless => false,
        ByName::Other => return Routed::NoReader,
    };
    match boxt_read_cpp::identify(bytes, ctx, listed) {
        Ok(true) => Routed::Cxx,
        Ok(false) => Routed::NoReader,
        Err(not_read) => Routed::NotRead(not_read, listed.then_some(FileKind::Source)),
    }
}
