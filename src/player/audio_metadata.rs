use std::{io::Cursor, path::Path};

use image::{ImageReader, Limits};
use lofty::{
    config::ParseOptions,
    picture::{Picture, PictureType},
    prelude::*,
    probe::Probe,
    tag::Tag,
};

const ARTWORK_MAX_DIMENSION: u32 = 1_024;
const ARTWORK_DECODE_MAX_DIMENSION: u32 = 8_192;
const ARTWORK_DECODE_MAX_BYTES: u64 = 64 * 1024 * 1024;

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
        let Ok(probe) = Probe::open(path) else {
            return Self::default();
        };
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
    value.trim().to_owned()
}

fn select_artwork(tags: &[Tag]) -> Option<&Picture> {
    tags.iter()
        .flat_map(|tag| tag.pictures())
        .find(|picture| picture.pic_type() == PictureType::CoverFront)
        .or_else(|| tags.iter().flat_map(|tag| tag.pictures()).next())
}

fn decode_artwork(picture: &Picture) -> Option<DecodedArtwork> {
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
    use super::clean_field;

    #[test]
    fn trims_metadata_fields() {
        assert_eq!(clean_field("  Track title \r\n".into()), "Track title");
    }
}
