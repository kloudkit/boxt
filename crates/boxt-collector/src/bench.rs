use std::io;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use boxt_core::compile_db::extension_class;
use boxt_core::location::InputRef;
use boxt_core::message::{Job, JobFile, JobKind};
use boxt_core::outcome::line_count;
use boxt_read_cpp::Grammar;

use crate::path::{job_path, rel_path, text_from_path};

pub struct Walked {
    pub jobs: Vec<Job>,
    pub skipped: usize,
}

pub fn cxx_jobs(root: &Path) -> io::Result<Walked> {
    let input = InputRef {
        index: 0,
        given: text_from_path(root),
    };
    let mut files = Vec::new();
    let mut skipped = 0;
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            let path = entry.path();
            if kind.is_dir() {
                dirs.push(path);
            } else if kind.is_file() {
                match rel_path(root, &path) {
                    Some(rel) if extension_class(rel.file_name()).is_some() => files.push(rel),
                    Some(_) => {}
                    None => skipped += 1,
                }
            }
        }
    }
    files.sort();
    let jobs = files
        .into_iter()
        .map(|rel| Job {
            kind: JobKind::Route,
            input: input.clone(),
            file: JobFile::InInput(rel),
            resume_after: None,
        })
        .collect();
    Ok(Walked { jobs, skipped })
}

pub fn parse_in_process(jobs: &[Job], threads: NonZeroUsize) -> io::Result<usize> {
    let next = AtomicUsize::new(0);
    let read = AtomicUsize::new(0);
    thread::scope(|scope| {
        let workers: Vec<_> = (0..threads.get())
            .map(|_| {
                scope.spawn(|| -> io::Result<()> {
                    while let Some(job) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                        if parse_one(job)? {
                            read.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Ok(())
                })
            })
            .collect();
        workers.into_iter().try_for_each(|w| {
            w.join()
                .unwrap_or_else(|_| Err(io::Error::other("a parse thread panicked")))
        })
    })?;
    Ok(read.into_inner())
}

fn parse_one(job: &Job) -> io::Result<bool> {
    let Some(path) = job_path(job) else {
        return Ok(false);
    };
    let Some(grammar) = job
        .path()
        .and_then(|p| Grammar::for_file_name(p.file_name()))
    else {
        return Ok(false);
    };
    let bytes = std::fs::read(path)?;
    std::hint::black_box(line_count(&bytes, None));
    Ok(boxt_read_cpp::parse(&bytes, grammar).is_ok())
}
