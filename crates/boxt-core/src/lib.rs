#![forbid(unsafe_code)]

macro_rules! count {
    () => { 0usize };
    ($head:ident $($tail:ident)*) => { 1usize + count!($($tail)*) };
}

macro_rules! closed_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $($(#[$vmeta:meta])* $variant:ident,)+
        }
    ) => {
        $(#[$meta])*
        pub enum $name {
            $($(#[$vmeta])* $variant,)+
        }

        impl $name {
            pub const ALL: [$name; count!($($variant)+)] = [$($name::$variant,)+];
        }
    };
}

pub mod compile_db;
pub mod fact;
pub mod limits;
pub mod location;
pub mod message;
pub mod outcome;
pub mod settings;
pub mod shell;
pub mod sorted;
pub mod text;
pub mod verdict;
pub mod version;

pub use sorted::SortedVec;
pub use text::Text;
