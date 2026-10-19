//! DJ-ready exports: format conversion, tags and artwork. The source file is
//! only ever read; every export writes a new file.

pub mod convert;
pub mod id3;
pub mod mp3;
pub mod picture;
pub mod writer;
