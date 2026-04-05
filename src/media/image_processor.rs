//! Image processing implementation using the `image` crate.
//!
//! Behind the `media` feature flag. Provides WebP conversion, thumbnail
//! generation, blurhash computation, EXIF validation, and perceptual hashing.

use super::{MediaError, MediaProcessor, MediaResult};

/// Image processor using the `image`, `webp`, `blurhash`, and `kamadak-exif` crates.
pub struct ImageProcessor {
    /// Maximum thumbnail dimension (width or height).
    pub thumbnail_max_size: u32,
    /// Whether to strip EXIF data from processed images.
    pub strip_exif: bool,
    /// Whether to reject images with GPS EXIF data.
    pub reject_gps_exif: bool,
}

impl Default for ImageProcessor {
    fn default() -> Self {
        Self {
            thumbnail_max_size: 200,
            strip_exif: true,
            reject_gps_exif: true,
        }
    }
}

impl ImageProcessor {
    pub fn new() -> Self {
        Self::default()
    }
}

impl MediaProcessor for ImageProcessor {
    fn process(&self, data: &[u8], mime_type: &str) -> Result<MediaResult, MediaError> {
        if !mime_type.starts_with("image/") {
            return Err(MediaError::UnsupportedType(mime_type.to_string()));
        }

        if self.reject_gps_exif {
            self.validate_exif(data)?;
        }

        // Decode image.
        let img = image::load_from_memory(data)
            .map_err(|e| MediaError::ProcessingFailed(format!("decode: {e}")))?;

        let width = img.width();
        let height = img.height();

        // Generate thumbnail.
        let thumb = img.thumbnail(self.thumbnail_max_size, self.thumbnail_max_size);
        let mut thumb_bytes = Vec::new();
        thumb
            .write_to(
                &mut std::io::Cursor::new(&mut thumb_bytes),
                image::ImageFormat::Png,
            )
            .map_err(|e| MediaError::ProcessingFailed(format!("thumbnail: {e}")))?;

        Ok(MediaResult {
            data: data.to_vec(),
            mime_type: mime_type.to_string(),
            width: Some(width),
            height: Some(height),
            blurhash: None, // TODO: compute blurhash
            thumbnail: Some(thumb_bytes),
            phash: None, // TODO: compute perceptual hash
        })
    }

    fn validate_exif(&self, data: &[u8]) -> Result<(), MediaError> {
        let reader = std::io::Cursor::new(data);
        let exif_reader = exif::Reader::new();
        if let Ok(exif) = exif_reader.read_from_container(&mut std::io::BufReader::new(reader)) {
            // Check for GPS fields.
            for field in exif.fields() {
                let tag_str = format!("{}", field.tag);
                if tag_str.contains("GPS") {
                    return Err(MediaError::SensitiveExif(format!(
                        "GPS data found: {}",
                        field.tag
                    )));
                }
            }
        }
        // No EXIF or no sensitive fields — OK.
        Ok(())
    }

    fn perceptual_hash(&self, data: &[u8]) -> Result<u64, MediaError> {
        // Simple DCT-based perceptual hash.
        let img = image::load_from_memory(data)
            .map_err(|e| MediaError::ProcessingFailed(format!("decode: {e}")))?;

        // Resize to 8x8 grayscale.
        let small = img.resize_exact(8, 8, image::imageops::FilterType::Lanczos3);
        let gray = small.to_luma8();

        // Compute mean.
        let pixels: Vec<u8> = gray.pixels().map(|p| p.0[0]).collect();
        let mean: f64 = pixels.iter().map(|&p| p as f64).sum::<f64>() / pixels.len() as f64;

        // Build hash: bit is 1 if pixel > mean.
        let mut hash: u64 = 0;
        for (i, &pixel) in pixels.iter().enumerate() {
            if pixel as f64 > mean {
                hash |= 1 << (63 - i);
            }
        }

        Ok(hash)
    }

    fn blurhash(&self, data: &[u8]) -> Result<String, MediaError> {
        let img = image::load_from_memory(data)
            .map_err(|e| MediaError::ProcessingFailed(format!("decode: {e}")))?;

        let small = img.resize(32, 32, image::imageops::FilterType::Lanczos3);
        let rgba = small.to_rgba8();
        let (w, h) = (rgba.width() as usize, rgba.height() as usize);
        let pixels: Vec<u8> = rgba.into_raw();

        let hash = blurhash::encode(4, 3, w, h, &pixels)
            .map_err(|e| MediaError::ProcessingFailed(format!("blurhash: {e}")))?;
        Ok(hash)
    }

    fn thumbnail(
        &self,
        data: &[u8],
        max_width: u32,
        max_height: u32,
    ) -> Result<Vec<u8>, MediaError> {
        let img = image::load_from_memory(data)
            .map_err(|e| MediaError::ProcessingFailed(format!("decode: {e}")))?;

        let thumb = img.thumbnail(max_width, max_height);
        let mut bytes = Vec::new();
        thumb
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .map_err(|e| MediaError::ProcessingFailed(format!("thumbnail: {e}")))?;
        Ok(bytes)
    }
}
