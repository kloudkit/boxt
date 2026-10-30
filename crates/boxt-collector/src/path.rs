use std::path::{Path, PathBuf};

use boxt_core::Text;
use boxt_core::location::RelPath;
use boxt_core::message::{Job, JobFile};

#[cfg(unix)]
pub fn text_from_path(path: &Path) -> Text {
    use std::os::unix::ffi::OsStrExt;
    Text::from_bytes(path.as_os_str().as_bytes().to_vec())
}

#[cfg(windows)]
pub fn text_from_path(path: &Path) -> Text {
    use std::os::windows::ffi::OsStrExt;
    Text::from_wide(&path.as_os_str().encode_wide().collect::<Vec<u16>>())
}

#[cfg(unix)]
pub fn path_from_text(text: &Text) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(text.as_bytes())))
}

#[cfg(windows)]
pub fn path_from_text(text: &Text) -> Option<PathBuf> {
    text.as_str().map(PathBuf::from)
}

pub fn rel_path(root: &Path, file: &Path) -> Option<RelPath> {
    let rel = file.strip_prefix(root).ok()?;
    let mut bytes = Vec::new();
    for part in rel.components() {
        if !bytes.is_empty() {
            bytes.push(b'/');
        }
        bytes.extend_from_slice(text_from_path(Path::new(part.as_os_str())).as_bytes());
    }
    RelPath::new(Text::from_bytes(bytes)).ok()
}

pub fn job_path(job: &Job) -> Option<PathBuf> {
    let given = path_from_text(&job.input.given)?;
    match &job.file {
        JobFile::WholeInput(_) => Some(given),
        JobFile::InInput(rel) => Some(given.join(path_from_text(rel.as_text())?)),
        JobFile::Setting(file) => path_from_text(file),
    }
}
