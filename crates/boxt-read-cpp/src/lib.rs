//! C and C++ source reader: tree-sitter, guard structure and encodings.

use std::ffi::c_void;
use std::ops::ControlFlow;

use boxt_core::compile_db::extension_class;
use tree_sitter::{Language, ParseOptions, Parser};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grammar {
    C,
    Cpp,
}

impl Grammar {
    pub fn for_file_name(file_name: &[u8]) -> Option<Grammar> {
        extension_class(file_name)?;
        let dot = file_name.iter().rposition(|&c| c == b'.')?;
        let ext = &file_name[dot + 1..];
        if ext.eq_ignore_ascii_case(b"c") || ext.eq_ignore_ascii_case(b"h") {
            Some(Grammar::C)
        } else {
            Some(Grammar::Cpp)
        }
    }

    fn language(self) -> Language {
        match self {
            Grammar::C => tree_sitter_c::LANGUAGE.into(),
            Grammar::Cpp => tree_sitter_cpp::LANGUAGE.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseSummary {
    pub has_error: bool,
    pub steps: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    Language,
    Cancelled,
}

pub fn parse(bytes: &[u8], grammar: Grammar) -> Result<ParseSummary, ParseError> {
    let mut parser = Parser::new();
    parser
        .set_language(&grammar.language())
        .map_err(|_| ParseError::Language)?;
    let mut steps = 0u64;
    let mut progress = |_: &tree_sitter::ParseState| {
        steps += 1;
        ControlFlow::Continue(())
    };
    let mut read = |i: usize, _| &bytes[i.min(bytes.len())..];
    let tree = parser
        .parse_with_options(
            &mut read,
            None,
            Some(ParseOptions::new().progress_callback(&mut progress)),
        )
        .ok_or(ParseError::Cancelled)?;
    Ok(ParseSummary {
        has_error: tree.root_node().has_error(),
        steps,
    })
}

pub type CMalloc = unsafe extern "C" fn(usize) -> *mut c_void;
pub type CCalloc = unsafe extern "C" fn(usize, usize) -> *mut c_void;
pub type CRealloc = unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void;
pub type CFree = unsafe extern "C" fn(*mut c_void);

/// # Safety
///
/// Call before any other tree-sitter call, from one thread, with four functions of one
/// allocator family that never return null for a non-zero size.
pub unsafe fn set_c_allocator(malloc: CMalloc, calloc: CCalloc, realloc: CRealloc, free: CFree) {
    unsafe {
        tree_sitter::set_allocator(Some(tree_sitter::Allocator {
            malloc,
            calloc,
            realloc,
            free,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar_follows_the_extension() {
        assert_eq!(Grammar::for_file_name(b"a/b.c"), Some(Grammar::C));
        assert_eq!(Grammar::for_file_name(b"b.H"), Some(Grammar::C));
        assert_eq!(Grammar::for_file_name(b"b.cpp"), Some(Grammar::Cpp));
        assert_eq!(Grammar::for_file_name(b"b.hpp"), Some(Grammar::Cpp));
        assert_eq!(Grammar::for_file_name(b"b.txt"), None);
        assert_eq!(Grammar::for_file_name(b"Makefile"), None);
    }

    #[test]
    fn clean_and_broken_sources() {
        let ok = parse(b"int main(void) { return f(1); }\n", Grammar::C).unwrap();
        assert!(!ok.has_error);
        let bad = parse(b"int main( { return\n", Grammar::C).unwrap();
        assert!(bad.has_error);
        let cpp = parse(b"namespace a { class B { int f(); }; }\n", Grammar::Cpp).unwrap();
        assert!(!cpp.has_error);
    }
}
