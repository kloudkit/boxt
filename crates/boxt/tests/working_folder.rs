mod common;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use common::{seed_dir, verdict};

struct RestoreFolder(PathBuf);

impl Drop for RestoreFolder {
    fn drop(&mut self) {
        let _ = env::set_current_dir(&self.0);
    }
}

fn copy_seed(to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(seed_dir()).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
    }
}

#[test]
fn the_result_is_byte_identical_from_two_working_folders() {
    let base = Path::new(env!("CARGO_TARGET_TMPDIR")).join("verdict-working-folder");
    let _ = fs::remove_dir_all(&base);
    let first_folder = base.join("first");
    let second_folder = base.join("second").join("deeper");
    copy_seed(&first_folder.join("seed"));
    copy_seed(&second_folder.join("seed"));
    let relative = [PathBuf::from("seed")];
    let _restore = RestoreFolder(env::current_dir().unwrap());
    env::set_current_dir(&first_folder).unwrap();
    let first = serde_json::to_vec(&verdict(&relative)).unwrap();
    env::set_current_dir(&second_folder).unwrap();
    let second = serde_json::to_vec(&verdict(&relative)).unwrap();
    assert!(!first.is_empty());
    assert_eq!(first, second);
}
