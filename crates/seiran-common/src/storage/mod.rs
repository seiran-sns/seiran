pub mod image;
pub mod s3;
pub mod selector;

pub use image::{process_image, ImageProcessingError, MediaKind, ProcessedImage};
pub use s3::{S3StorageClient, S3Error};
pub use selector::{select_provider, SelectorError};
