use std::fs;
use std::io;
use std::path::Path;

const FILE_MODE: u32 = 0o644;

pub fn pack(dir: &Path) -> io::Result<Vec<u8>> {
    let mut files = Vec::new();
    collect(dir, "", &mut files)?;
    files.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    let mut builder = tar::Builder::new(Vec::new());
    for rel in &files {
        let data = fs::read(dir.join(rel))?;
        let mut header = tar::Header::new_ustar();
        header.set_path(rel)?;
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(FILE_MODE);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_size(data.len() as u64);
        header.set_cksum();
        builder.append(&header, data.as_slice())?;
    }
    builder.into_inner()
}

fn collect(root: &Path, prefix: &str, files: &mut Vec<String>) -> io::Result<()> {
    let dir = if prefix.is_empty() {
        root.to_path_buf()
    } else {
        root.join(prefix)
    };
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().into_string().map_err(|name| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("corpus entry {name:?} is not UTF-8"),
            )
        })?;
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect(root, &rel, files)?;
        } else if kind.is_file() {
            files.push(rel);
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("corpus entry {rel} is neither a file nor a directory"),
            ));
        }
    }
    Ok(())
}
