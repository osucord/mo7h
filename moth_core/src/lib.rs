#![warn(clippy::pedantic)]
// clippy warns for casting, but as i cast data into the database and out its fine.
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::module_name_repetitions,
    clippy::unreadable_literal,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation
)]

pub mod config;
pub mod data;
pub mod emojis;
pub mod ocr;
pub mod standby;
pub mod verification;
