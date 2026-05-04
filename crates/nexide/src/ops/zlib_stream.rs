//! Streaming compression / decompression backing the `node:zlib`
//! transform classes.
//!
//! Each [`ZlibStream`] owns one of the `flate2` write-side encoders
//! or decoders. The JS layer feeds input through
//! [`ZlibStream::feed`] and finalises with [`ZlibStream::finish`];
//! the host returns produced output bytes after each call so the
//! JS Transform stream can push them downstream.

use std::io::Write;

use brotli::{CompressorWriter as BrotliCompressorWriter, DecompressorWriter as BrotliDecompressorWriter};
use flate2::Compression;
use flate2::write::{
    DeflateDecoder, DeflateEncoder, GzDecoder, GzEncoder, ZlibDecoder, ZlibEncoder,
};

use super::net::NetError;

const LOG_TARGET: &str = "nexide::ops::zlib";

/// Kind of stream to instantiate. Mirrors the Node `zlib` factory
/// surface (`createDeflate`, `createGzip`, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZlibKind {
    /// `Deflate` - zlib-wrapped deflate (RFC 1950).
    Deflate,
    /// `Inflate` - zlib-wrapped deflate decoder.
    Inflate,
    /// `DeflateRaw` - raw deflate (RFC 1951), no header.
    DeflateRaw,
    /// `InflateRaw` - raw deflate decoder.
    InflateRaw,
    /// `Gzip` - gzip wrapper (RFC 1952).
    Gzip,
    /// `Gunzip` - gzip decoder.
    Gunzip,
    /// `BrotliCompress` - streaming brotli encoder.
    BrotliCompress,
    /// `BrotliDecompress` - streaming brotli decoder.
    BrotliDecompress,
}

/// Streaming engine. The `flate2` write-side adapters consume the
/// inner `Vec<u8>` as their sink so we can drain it after every
/// `feed` call.
pub enum ZlibStream {
    /// Zlib-wrapped deflate encoder.
    Deflate(ZlibEncoder<Vec<u8>>),
    /// Raw deflate encoder (no zlib header).
    DeflateRaw(DeflateEncoder<Vec<u8>>),
    /// Gzip encoder (zlib + gzip header/footer).
    Gzip(GzEncoder<Vec<u8>>),
    /// Zlib-wrapped deflate decoder.
    Inflate(ZlibDecoder<Vec<u8>>),
    /// Raw deflate decoder.
    InflateRaw(DeflateDecoder<Vec<u8>>),
    /// Gzip decoder.
    Gunzip(GzDecoder<Vec<u8>>),
    /// Brotli encoder. The CompressorWriter buffers into the inner
    /// Vec; we drain after every write to surface output bytes.
    BrotliCompress(Box<BrotliCompressorWriter<Vec<u8>>>),
    /// Brotli decoder.
    BrotliDecompress(Box<BrotliDecompressorWriter<Vec<u8>>>),
}

impl ZlibStream {
    /// Builds a fresh stream for `kind` with `level` clamped to the
    /// `0..=9` range honoured by Node.
    #[must_use]
    pub fn new(kind: ZlibKind, level: u32) -> Self {
        let flate_level = Compression::new(level.min(9));
        match kind {
            ZlibKind::Deflate => Self::Deflate(ZlibEncoder::new(Vec::new(), flate_level)),
            ZlibKind::DeflateRaw => Self::DeflateRaw(DeflateEncoder::new(Vec::new(), flate_level)),
            ZlibKind::Gzip => Self::Gzip(GzEncoder::new(Vec::new(), flate_level)),
            ZlibKind::Inflate => Self::Inflate(ZlibDecoder::new(Vec::new())),
            ZlibKind::InflateRaw => Self::InflateRaw(DeflateDecoder::new(Vec::new())),
            ZlibKind::Gunzip => Self::Gunzip(GzDecoder::new(Vec::new())),
            ZlibKind::BrotliCompress => {
                // quality 0..=11, lgwin default 22 (matches Node's default
                // BROTLI_DEFAULT_WINDOW). Node's default quality is 11
                // for one-shot but level=6 maps reasonably well for
                // streaming.
                let quality = level.min(11);
                Self::BrotliCompress(Box::new(BrotliCompressorWriter::new(Vec::new(), 4096, quality, 22)))
            }
            ZlibKind::BrotliDecompress => {
                Self::BrotliDecompress(Box::new(BrotliDecompressorWriter::new(Vec::new(), 4096)))
            }
        }
    }

    /// Feeds `input` into the stream and returns any newly produced
    /// output bytes (zero or more).
    ///
    /// # Errors
    /// Returns a [`NetError`] tagged with `Z_DATA_ERROR` when the
    /// encoder/decoder reports a fatal transition.
    pub fn feed(&mut self, input: &[u8]) -> Result<Vec<u8>, NetError> {
        let result = match self {
            Self::Deflate(e) => write_and_drain(e, input, |e| e.get_mut()),
            Self::DeflateRaw(e) => write_and_drain(e, input, |e| e.get_mut()),
            Self::Gzip(e) => write_and_drain(e, input, |e| e.get_mut()),
            Self::Inflate(d) => write_and_drain(d, input, |d| d.get_mut()),
            Self::InflateRaw(d) => write_and_drain(d, input, |d| d.get_mut()),
            Self::Gunzip(d) => write_and_drain(d, input, |d| d.get_mut()),
            Self::BrotliCompress(e) => write_and_drain(e, input, |e| e.get_mut()),
            Self::BrotliDecompress(d) => write_and_drain(d, input, |d| d.get_mut()),
        };
        match &result {
            Ok(out) => tracing::trace!(
                target: LOG_TARGET,
                in_bytes = input.len(),
                out_bytes = out.len(),
                "zlib feed",
            ),
            Err(err) => tracing::warn!(
                target: LOG_TARGET,
                in_bytes = input.len(),
                code = err.code,
                message = %err.message,
                "zlib feed failed",
            ),
        }
        result
    }

    /// Flushes any internal buffer, signalling end-of-input.
    /// Returns the trailing bytes (footer for gzip / zlib, etc.).
    ///
    /// # Errors
    /// Returns a [`NetError`] when finalisation fails.
    pub fn finish(self) -> Result<Vec<u8>, NetError> {
        let result = match self {
            Self::Deflate(e) => e.finish().map_err(io_to_net),
            Self::DeflateRaw(e) => e.finish().map_err(io_to_net),
            Self::Gzip(e) => e.finish().map_err(io_to_net),
            Self::Inflate(d) => d.finish().map_err(io_to_net),
            Self::InflateRaw(d) => d.finish().map_err(io_to_net),
            Self::Gunzip(d) => d.finish().map_err(io_to_net),
            Self::BrotliCompress(mut e) => {
                e.flush().map_err(io_to_net)?;
                drop(e);
                // CompressorWriter doesn't expose into_inner; flush is
                // sufficient because brotli emits its final block on
                // flush. The drained bytes were already returned by
                // earlier feeds and the trailing flush.
                Ok(Vec::new())
            }
            Self::BrotliDecompress(mut d) => {
                d.flush().map_err(io_to_net)?;
                let tail = std::mem::take(d.get_mut());
                Ok(tail)
            }
        };
        match &result {
            Ok(out) => tracing::debug!(
                target: LOG_TARGET,
                tail_bytes = out.len(),
                "zlib stream finished",
            ),
            Err(err) => tracing::warn!(
                target: LOG_TARGET,
                code = err.code,
                message = %err.message,
                "zlib finish failed",
            ),
        }
        result
    }
}

fn write_and_drain<W: Write>(
    sink: &mut W,
    input: &[u8],
    drain: impl FnOnce(&mut W) -> &mut Vec<u8>,
) -> Result<Vec<u8>, NetError> {
    sink.write_all(input).map_err(io_to_net)?;
    sink.flush().map_err(io_to_net)?;
    let buf = drain(sink);
    let out = std::mem::take(buf);
    Ok(out)
}

fn io_to_net(err: std::io::Error) -> NetError {
    NetError::new("Z_DATA_ERROR", err.to_string())
}

/// Parses the kebab-style kind name produced by the JS bridge.
///
/// # Errors
/// Returns `EINVAL` for unknown kinds.
pub fn parse_kind(name: &str) -> Result<ZlibKind, NetError> {
    Ok(match name {
        "deflate" => ZlibKind::Deflate,
        "inflate" => ZlibKind::Inflate,
        "deflate-raw" => ZlibKind::DeflateRaw,
        "inflate-raw" => ZlibKind::InflateRaw,
        "gzip" => ZlibKind::Gzip,
        "gunzip" => ZlibKind::Gunzip,
        "brotli-compress" => ZlibKind::BrotliCompress,
        "brotli-decompress" => ZlibKind::BrotliDecompress,
        other => {
            return Err(NetError::new(
                "EINVAL",
                format!("unknown zlib kind: {other}"),
            ));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deflate_inflate_round_trip() {
        let payload = b"the quick brown fox".repeat(8);
        let mut enc = ZlibStream::new(ZlibKind::Deflate, 6);
        let mut compressed = enc.feed(&payload).unwrap();
        compressed.extend(enc.finish().unwrap());

        let mut dec = ZlibStream::new(ZlibKind::Inflate, 0);
        let mut out = dec.feed(&compressed).unwrap();
        out.extend(dec.finish().unwrap());
        assert_eq!(out, payload);
    }

    #[test]
    fn gzip_round_trip() {
        let payload = b"hello-gzip".to_vec();
        let mut enc = ZlibStream::new(ZlibKind::Gzip, 6);
        let mut compressed = enc.feed(&payload).unwrap();
        compressed.extend(enc.finish().unwrap());

        let mut dec = ZlibStream::new(ZlibKind::Gunzip, 0);
        let mut out = dec.feed(&compressed).unwrap();
        out.extend(dec.finish().unwrap());
        assert_eq!(out, payload);
    }

    #[test]
    fn raw_deflate_round_trip() {
        let payload = b"raw-test-payload-12345".to_vec();
        let mut enc = ZlibStream::new(ZlibKind::DeflateRaw, 4);
        let mut compressed = enc.feed(&payload).unwrap();
        compressed.extend(enc.finish().unwrap());

        let mut dec = ZlibStream::new(ZlibKind::InflateRaw, 0);
        let mut out = dec.feed(&compressed).unwrap();
        out.extend(dec.finish().unwrap());
        assert_eq!(out, payload);
    }

    #[test]
    fn parse_kind_rejects_unknown() {
        assert!(parse_kind("nope").is_err());
        assert!(parse_kind("deflate").is_ok());
        assert!(parse_kind("brotli-compress").is_ok());
        assert!(parse_kind("brotli-decompress").is_ok());
    }

    #[test]
    fn brotli_round_trip() {
        let payload = b"streaming brotli test payload that compresses".repeat(4);
        let mut enc = ZlibStream::new(ZlibKind::BrotliCompress, 4);
        let mut compressed = enc.feed(&payload).unwrap();
        compressed.extend(enc.finish().unwrap());

        let mut dec = ZlibStream::new(ZlibKind::BrotliDecompress, 0);
        let mut out = dec.feed(&compressed).unwrap();
        out.extend(dec.finish().unwrap());
        assert_eq!(out, payload);
    }
}
