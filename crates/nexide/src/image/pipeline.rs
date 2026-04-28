//! Decode → resize → encode pipeline.
//!
//! Pure CPU work; no I/O. Source-format detection uses the `image`
//! crate's magic-byte sniffer, which mirrors upstream's content-type
//! detection table for every format we accept (png, jpeg, gif, webp).

use std::io::Cursor;

use fast_image_resize::{
    PixelType, ResizeAlg, ResizeOptions, Resizer,
    images::{Image, ImageRef},
};
use image::codecs::{jpeg::JpegEncoder, png::PngEncoder, webp::WebPEncoder};
use image::{DynamicImage, ImageEncoder, ImageFormat, ImageReader};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceFormat {
    Jpeg,
    Png,
    Gif,
    Webp,
    Svg,
    Bmp,
    Ico,
    Icns,
    Heic,
    Jxl,
    Avif,
    Unknown,
}

impl SourceFormat {
    pub(crate) const fn mime(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
            Self::Svg => "image/svg+xml",
            Self::Bmp => "image/bmp",
            Self::Ico => "image/x-icon",
            Self::Icns => "image/x-icns",
            Self::Heic => "image/heic",
            Self::Jxl => "image/jxl",
            Self::Avif => "image/avif",
            Self::Unknown => "application/octet-stream",
        }
    }

    pub(crate) const fn is_bypass(self) -> bool {
        matches!(
            self,
            Self::Svg | Self::Bmp | Self::Ico | Self::Icns | Self::Heic | Self::Jxl
        )
    }

    pub(crate) const fn is_image(self) -> bool {
        !matches!(self, Self::Unknown)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputFormat {
    Webp,
    Jpeg,
    Png,
}

impl OutputFormat {
    pub(crate) const fn mime(self) -> &'static str {
        match self {
            Self::Webp => "image/webp",
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
        }
    }

    pub(crate) const fn extension(self) -> &'static str {
        match self {
            Self::Webp => "webp",
            Self::Jpeg => "jpg",
            Self::Png => "png",
        }
    }

    pub(crate) fn from_mime(mime: &str) -> Option<Self> {
        match mime {
            "image/webp" => Some(Self::Webp),
            "image/jpeg" => Some(Self::Jpeg),
            "image/png" => Some(Self::Png),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PipelineError {
    #[error("image codec error: {0}")]
    Codec(#[from] image::ImageError),
    #[error("resize error: {0}")]
    Resize(String),
    #[error("source has zero dimension")]
    EmptyDimension,
}

/// Sniffs the source format from raw bytes using magic byte signatures
/// (mirror of `detectContentType` in image-optimizer.js).
pub(crate) fn detect_format(bytes: &[u8]) -> SourceFormat {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return SourceFormat::Jpeg;
    }
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return SourceFormat::Png;
    }
    if bytes.starts_with(b"GIF8") {
        return SourceFormat::Gif;
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return SourceFormat::Webp;
    }
    if bytes.starts_with(b"<?xml") || trimmed_starts_with(bytes, b"<svg") {
        return SourceFormat::Svg;
    }
    if bytes.starts_with(b"BM") {
        return SourceFormat::Bmp;
    }
    if bytes.starts_with(&[0x00, 0x00, 0x01, 0x00]) {
        return SourceFormat::Ico;
    }
    if bytes.starts_with(b"icns") {
        return SourceFormat::Icns;
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        let brand = &bytes[8..12];
        if brand == b"avif" || brand == b"avis" {
            return SourceFormat::Avif;
        }
        if brand == b"heic" || brand == b"heix" || brand == b"mif1" || brand == b"msf1" {
            return SourceFormat::Heic;
        }
    }
    if bytes.starts_with(&[0xFF, 0x0A])
        || bytes.starts_with(&[0x00, 0x00, 0x00, 0x0C, b'J', b'X', b'L', b' '])
    {
        return SourceFormat::Jxl;
    }
    SourceFormat::Unknown
}

fn trimmed_starts_with(haystack: &[u8], needle: &[u8]) -> bool {
    let mut i = 0;
    while i < haystack.len()
        && (haystack[i] == b' '
            || haystack[i] == b'\t'
            || haystack[i] == b'\n'
            || haystack[i] == b'\r')
    {
        i += 1;
    }
    haystack[i..].starts_with(needle)
}

/// Decodes the source bytes into a [`DynamicImage`].
pub(crate) fn decode(bytes: &[u8]) -> Result<DynamicImage, PipelineError> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(image::ImageError::from)?;
    let img = reader.decode()?;
    Ok(img)
}

/// Resizes `img` to fit within `target_w` while preserving aspect
/// ratio. Mirrors upstream's `withoutEnlargement: true`: never upscales.
pub(crate) fn resize(img: &DynamicImage, target_w: u32) -> Result<DynamicImage, PipelineError> {
    if img.width() == 0 || img.height() == 0 {
        return Err(PipelineError::EmptyDimension);
    }
    if target_w >= img.width() {
        return Ok(img.clone());
    }
    let target_h =
        ((u64::from(target_w) * u64::from(img.height())) / u64::from(img.width())) as u32;
    let target_h = target_h.max(1);

    let rgba = img.to_rgba8();
    let src = ImageRef::new(rgba.width(), rgba.height(), rgba.as_raw(), PixelType::U8x4)
        .map_err(|e| PipelineError::Resize(e.to_string()))?;
    let mut dst = Image::new(target_w, target_h, PixelType::U8x4);
    let mut resizer = Resizer::new();
    resizer
        .resize(
            &src,
            &mut dst,
            &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(
                fast_image_resize::FilterType::Lanczos3,
            )),
        )
        .map_err(|e| PipelineError::Resize(e.to_string()))?;
    let buf = image::RgbaImage::from_raw(target_w, target_h, dst.into_vec())
        .ok_or(PipelineError::EmptyDimension)?;
    Ok(DynamicImage::ImageRgba8(buf))
}

/// Encodes `img` to `format` at `quality` (0..=100). PNG/WebP ignore
/// the quality value because the encoders we use are lossless; JPEG
/// honours it via `JpegEncoder::new_with_quality`.
pub(crate) fn encode(
    img: &DynamicImage,
    format: OutputFormat,
    quality: u8,
) -> Result<Vec<u8>, PipelineError> {
    let mut out = Vec::with_capacity(64 * 1024);
    match format {
        OutputFormat::Webp => {
            let rgba = img.to_rgba8();
            let encoder = WebPEncoder::new_lossless(&mut out);
            encoder.write_image(
                &rgba,
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )?;
        }
        OutputFormat::Jpeg => {
            let rgb = img.to_rgb8();
            let mut encoder = JpegEncoder::new_with_quality(&mut out, quality);
            encoder.encode_image(&rgb)?;
        }
        OutputFormat::Png => {
            let rgba = img.to_rgba8();
            let encoder = PngEncoder::new(&mut out);
            encoder.write_image(
                &rgba,
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )?;
        }
    }
    Ok(out)
}

/// Negotiates an output format. Mirrors the three-tier fallback in
/// `image-optimizer.js:1097-1104`:
///   1. Negotiated MIME from `Accept` ∩ `formats` config.
///   2. Otherwise the source's own format if it can be re-encoded.
///   3. Otherwise JPEG.
pub(crate) fn choose_output(
    source: SourceFormat,
    accept: &str,
    formats: &[String],
) -> OutputFormat {
    if let Some(negotiated) = negotiate_accept(accept, formats)
        && let Some(fmt) = OutputFormat::from_mime(&negotiated)
    {
        return fmt;
    }
    match source {
        SourceFormat::Webp => OutputFormat::Webp,
        SourceFormat::Png => OutputFormat::Png,
        SourceFormat::Gif | SourceFormat::Jpeg => OutputFormat::Jpeg,
        _ => OutputFormat::Jpeg,
    }
}

fn negotiate_accept(accept: &str, formats: &[String]) -> Option<String> {
    if accept.is_empty() {
        return None;
    }
    let offered: Vec<&str> = formats.iter().map(String::as_str).collect();
    for offer in &offered {
        if accept_contains(accept, offer) {
            return Some((*offer).to_owned());
        }
    }
    None
}

fn accept_contains(accept: &str, mime: &str) -> bool {
    accept
        .split(',')
        .map(|tok| tok.split(';').next().unwrap_or("").trim())
        .any(|t| t.eq_ignore_ascii_case(mime))
}

/// Maps an [`ImageFormat`] returned by the `image` crate into the
/// matching [`SourceFormat`]; used when we have already decoded.
#[allow(dead_code)]
pub(crate) const fn from_image_format(fmt: ImageFormat) -> SourceFormat {
    match fmt {
        ImageFormat::Png => SourceFormat::Png,
        ImageFormat::Jpeg => SourceFormat::Jpeg,
        ImageFormat::Gif => SourceFormat::Gif,
        ImageFormat::WebP => SourceFormat::Webp,
        ImageFormat::Bmp => SourceFormat::Bmp,
        _ => SourceFormat::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_png() {
        let bytes = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0];
        assert_eq!(detect_format(&bytes), SourceFormat::Png);
    }

    #[test]
    fn detects_jpeg() {
        let bytes = [0xFF, 0xD8, 0xFF, 0xE0];
        assert_eq!(detect_format(&bytes), SourceFormat::Jpeg);
    }

    #[test]
    fn detects_webp() {
        let bytes = [b'R', b'I', b'F', b'F', 0, 0, 0, 0, b'W', b'E', b'B', b'P'];
        assert_eq!(detect_format(&bytes), SourceFormat::Webp);
    }

    #[test]
    fn negotiates_webp_from_accept() {
        let formats = vec!["image/webp".to_owned()];
        let fmt = choose_output(SourceFormat::Png, "image/webp,*/*", &formats);
        assert_eq!(fmt, OutputFormat::Webp);
    }

    #[test]
    fn falls_back_to_source_when_no_match() {
        let formats = vec!["image/avif".to_owned()];
        let fmt = choose_output(SourceFormat::Png, "image/png", &formats);
        assert_eq!(fmt, OutputFormat::Png);
    }
}
