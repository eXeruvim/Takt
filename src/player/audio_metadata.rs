use std::{
    fs::File,
    io::{self, BufReader, Cursor, Read, Seek, SeekFrom},
    path::Path,
};

use image::{ImageReader, Limits};
use lofty::{
    config::{GlobalOptions, ParseOptions, apply_global_options},
    picture::{Picture, PictureType},
    prelude::*,
    probe::Probe,
    tag::Tag,
};

const ARTWORK_MAX_DIMENSION: u32 = 1_024;
const ARTWORK_SOURCE_MAX_BYTES: usize = 8 * 1024 * 1024;
const ARTWORK_DECODE_MAX_DIMENSION: u32 = 4_096;
const ARTWORK_DECODE_MAX_BYTES: u64 = 64 * 1024 * 1024;
const METADATA_READ_MAX_BYTES: u64 = 16 * 1024 * 1024;
const TAG_ITEM_MAX_BYTES: usize = 8 * 1024 * 1024;

use crate::player::limits::{UI_TEXT_BYTES, bounded_text};

struct ReadBudget<R> {
    inner: R,
    remaining: u64,
}

impl<R> ReadBudget<R> {
    fn new(inner: R, limit: u64) -> Self {
        Self {
            inner,
            remaining: limit,
        }
    }
}

impl<R: Read> Read for ReadBudget<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Метаданные превышают лимит чтения 16 МиБ",
            ));
        }
        let length = buffer.len().min(self.remaining as usize);
        let read = self.inner.read(&mut buffer[..length])?;
        self.remaining -= read as u64;
        Ok(read)
    }
}

impl<R: Seek> Seek for ReadBudget<R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.inner.seek(position)
    }
}

#[derive(Debug, Default)]
pub struct AudioMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub artwork: Option<DecodedArtwork>,
}

#[derive(Debug)]
pub struct DecodedArtwork {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl AudioMetadata {
    pub fn read(path: &Path) -> Self {
        // Lofty's allocation limit is per tag item and thread-local. The reader
        // budget additionally limits cumulative bytes consumed across all items.
        apply_global_options(
            GlobalOptions::new()
                .allocation_limit(TAG_ITEM_MAX_BYTES)
                .preserve_format_specific_items(false)
                .use_custom_resolvers(false),
        );
        let Ok(file) = File::open(path) else {
            return Self::default();
        };
        let reader = BufReader::new(ReadBudget::new(file, METADATA_READ_MAX_BYTES));
        let probe = Probe::new(reader);
        let Ok(probe) = probe.guess_file_type() else {
            return Self::default();
        };
        let Ok(tagged_file) = probe
            .options(ParseOptions::new().read_properties(false))
            .read()
        else {
            return Self::default();
        };

        let tag = tagged_file
            .primary_tag()
            .or_else(|| tagged_file.first_tag());
        let title = tag
            .and_then(Accessor::title)
            .map(clean_field)
            .unwrap_or_default();
        let artist = tag
            .and_then(Accessor::artist)
            .map(clean_field)
            .unwrap_or_default();
        let album = tag
            .and_then(Accessor::album)
            .map(clean_field)
            .unwrap_or_default();
        let artwork = select_artwork(tagged_file.tags()).and_then(decode_artwork);

        Self {
            title,
            artist,
            album,
            artwork,
        }
    }
}

fn clean_field(value: std::borrow::Cow<'_, str>) -> String {
    bounded_text(&value, UI_TEXT_BYTES)
}

fn select_artwork(tags: &[Tag]) -> Option<&Picture> {
    tags.iter()
        .flat_map(|tag| tag.pictures())
        .find(|picture| picture.pic_type() == PictureType::CoverFront)
        .or_else(|| tags.iter().flat_map(|tag| tag.pictures()).next())
}

fn decode_artwork(picture: &Picture) -> Option<DecodedArtwork> {
    if picture.data().len() > ARTWORK_SOURCE_MAX_BYTES {
        return None;
    }
    let mut reader = ImageReader::new(Cursor::new(picture.data()))
        .with_guessed_format()
        .ok()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(ARTWORK_DECODE_MAX_DIMENSION);
    limits.max_image_height = Some(ARTWORK_DECODE_MAX_DIMENSION);
    limits.max_alloc = Some(ARTWORK_DECODE_MAX_BYTES);
    reader.limits(limits);

    let image = reader.decode().ok()?;
    let image = image.thumbnail(ARTWORK_MAX_DIMENSION, ARTWORK_MAX_DIMENSION);
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();

    Some(DecodedArtwork {
        rgba: rgba.into_raw(),
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_metadata_fields() {
        assert_eq!(clean_field("  Track title \r\n".into()), "Track title");
    }

    #[test]
    fn metadata_fields_have_a_utf8_safe_payload_limit() {
        let field = clean_field(format!("  {}  ", "я".repeat(UI_TEXT_BYTES)).into());
        assert!(field.len() <= UI_TEXT_BYTES);
        assert!(field.ends_with('…'));
    }

    #[test]
    fn cumulative_reader_budget_is_not_reset_by_seeking() {
        let mut reader = ReadBudget::new(Cursor::new(*b"0123456789"), 4);
        let mut bytes = [0; 3];
        reader.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"012");
        reader.seek(SeekFrom::Start(0)).unwrap();
        assert_eq!(reader.read(&mut bytes).unwrap(), 1);
        assert_eq!(
            reader.read(&mut bytes).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn oversized_compressed_artwork_is_rejected_before_image_decode() {
        let picture = Picture::unchecked(vec![0; ARTWORK_SOURCE_MAX_BYTES + 1]).build();
        assert!(decode_artwork(&picture).is_none());
    }
}
