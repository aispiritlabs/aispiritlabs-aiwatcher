//! What a picture is, read from its first bytes.
//!
//! A hub answers "how big is this image" only for a column it has typed as an
//! image. A corpus that stores its pictures in a `binary` column — `content`,
//! `image_content`, `jpg`, whatever the uploader called it — hands over the
//! bytes and says nothing about them, and the registry requires a non-zero
//! width and height. So they are read here.
//!
//! Headers only. Decoding a JPEG to learn its size would mean a decoder, a
//! decompression-bomb policy and a dependency, to answer a question the first
//! two dozen bytes already answer. What this cannot read it says it cannot
//! read: [`describe`] returns `None` rather than a guess, and the caller
//! reports a column it could not measure instead of importing a zero.

/// The format and size of an image, as its header states them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pixels {
    pub content_type: &'static str,
    pub width: u32,
    pub height: u32,
}

/// What these bytes are, when they are a picture this can measure.
///
/// `None` for anything else — a PDF, a text blob, a TIFF, a truncated
/// download. Not an error: a `binary` column holding something that is not an
/// image is a normal corpus, and the caller's job is to skip that column
/// rather than to fail.
#[must_use]
pub fn describe(bytes: &[u8]) -> Option<Pixels> {
    png(bytes)
        .or_else(|| jpeg(bytes))
        .or_else(|| gif(bytes))
        .or_else(|| bmp(bytes))
        .or_else(|| webp(bytes))
}

fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    let slice: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
    Some(u32::from_be_bytes(slice))
}

fn le32(bytes: &[u8], at: usize) -> Option<u32> {
    let slice: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(slice))
}

fn be16(bytes: &[u8], at: usize) -> Option<u32> {
    let slice: [u8; 2] = bytes.get(at..at + 2)?.try_into().ok()?;
    Some(u32::from(u16::from_be_bytes(slice)))
}

fn le16(bytes: &[u8], at: usize) -> Option<u32> {
    let slice: [u8; 2] = bytes.get(at..at + 2)?.try_into().ok()?;
    Some(u32::from(u16::from_le_bytes(slice)))
}

fn png(bytes: &[u8]) -> Option<Pixels> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    Some(Pixels {
        content_type: "image/png",
        width: be32(bytes, 16)?,
        height: be32(bytes, 20)?,
    })
}

/// JPEG keeps its size in a start-of-frame segment, which sits after however
/// many other segments the encoder wrote first — so the segment chain has to be
/// walked rather than indexed into.
fn jpeg(bytes: &[u8]) -> Option<Pixels> {
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut at = 2;
    loop {
        // Fill bytes: any number of 0xFF may pad the gap before a marker.
        while bytes.get(at) == Some(&0xFF) && bytes.get(at + 1) == Some(&0xFF) {
            at += 1;
        }
        if bytes.get(at) != Some(&0xFF) {
            return None;
        }
        let marker = *bytes.get(at + 1)?;
        // Every start-of-frame except the four that are not frames: 0xC4 is a
        // Huffman table, 0xC8 is reserved, 0xCC an arithmetic-coding table.
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            return Some(Pixels {
                content_type: "image/jpeg",
                height: be16(bytes, at + 5)?,
                width: be16(bytes, at + 7)?,
            });
        }
        let length = be16(bytes, at + 2)? as usize;
        if length < 2 {
            return None;
        }
        at += 2 + length;
    }
}

fn gif(bytes: &[u8]) -> Option<Pixels> {
    if !bytes.starts_with(b"GIF87a") && !bytes.starts_with(b"GIF89a") {
        return None;
    }
    Some(Pixels {
        content_type: "image/gif",
        width: le16(bytes, 6)?,
        height: le16(bytes, 8)?,
    })
}

fn bmp(bytes: &[u8]) -> Option<Pixels> {
    if !bytes.starts_with(b"BM") {
        return None;
    }
    // A bottom-up bitmap states a negative height, and the sign is direction
    // rather than size.
    let height = le32(bytes, 22)? as i32;
    Some(Pixels {
        content_type: "image/bmp",
        width: le32(bytes, 18)?,
        height: height.unsigned_abs(),
    })
}

/// WebP has three chunk layouts and they carry the size in three places, which
/// is why this reads the chunk tag instead of one fixed offset.
fn webp(bytes: &[u8]) -> Option<Pixels> {
    if !bytes.starts_with(b"RIFF") || bytes.get(8..12) != Some(b"WEBP") {
        return None;
    }
    let content_type = "image/webp";
    match bytes.get(12..16)? {
        // Lossy: a 14-bit size after the start code.
        b"VP8 " => {
            let width = le16(bytes, 26)? & 0x3FFF;
            let height = le16(bytes, 28)? & 0x3FFF;
            Some(Pixels {
                content_type,
                width,
                height,
            })
        }
        // Lossless: 14 bits each, packed across four bytes, both minus one.
        b"VP8L" => {
            let packed = le32(bytes, 21)?;
            Some(Pixels {
                content_type,
                width: (packed & 0x3FFF) + 1,
                height: ((packed >> 14) & 0x3FFF) + 1,
            })
        }
        // Extended: 24 bits each, minus one, little-endian.
        b"VP8X" => {
            let width = u32::from(*bytes.get(24)?)
                | (u32::from(*bytes.get(25)?) << 8)
                | (u32::from(*bytes.get(26)?) << 16);
            let height = u32::from(*bytes.get(27)?)
                | (u32::from(*bytes.get(28)?) << 8)
                | (u32::from(*bytes.get(29)?) << 16);
            Some(Pixels {
                content_type,
                width: width + 1,
                height: height + 1,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_png_states_its_size_in_its_header() {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&[0; 8]);
        bytes.extend_from_slice(&1080_u32.to_be_bytes());
        bytes.extend_from_slice(&1537_u32.to_be_bytes());

        assert_eq!(
            describe(&bytes),
            Some(Pixels {
                content_type: "image/png",
                width: 1080,
                height: 1537,
            })
        );
    }

    /// The segment before the frame is the point: a JPEG that had its size read
    /// from a fixed offset would report whatever the encoder's comment happened
    /// to contain.
    #[test]
    fn a_jpeg_is_measured_after_walking_past_its_other_segments() {
        let mut bytes = vec![0xFF, 0xD8];
        // An APP0 segment, 16 bytes long, standing in for whatever an encoder
        // wrote first.
        bytes.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]);
        bytes.extend_from_slice(&[0; 14]);
        // SOF0: length, precision, then height and width.
        bytes.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
        bytes.extend_from_slice(&756_u16.to_be_bytes());
        bytes.extend_from_slice(&1080_u16.to_be_bytes());

        let found = describe(&bytes).expect("a jpeg");
        assert_eq!((found.width, found.height), (1080, 756));
        assert_eq!(found.content_type, "image/jpeg");
    }

    /// The whole reason this returns an `Option`. A `binary` column holding a
    /// PDF is a normal corpus — `pixparse/idl-wds` ships one beside its images
    /// — and importing it as a picture with a made-up size would be worse than
    /// skipping it.
    #[test]
    fn what_is_not_a_picture_is_not_measured() {
        assert_eq!(describe(b"%PDF-1.3\n%\xbf\xf7\xa2\xfe\n"), None);
        assert_eq!(describe(b"BROWN & WILLIAMSON TOBACCO"), None);
        assert_eq!(describe(&[]), None);
        // A truncated PNG: the magic matches and the header does not arrive.
        assert_eq!(describe(b"\x89PNG\r\n\x1a\n\x00\x00"), None);
    }

    #[test]
    fn a_bottom_up_bitmap_has_a_height_rather_than_a_direction() {
        let mut bytes = b"BM".to_vec();
        bytes.extend_from_slice(&[0; 16]);
        bytes.extend_from_slice(&640_u32.to_le_bytes());
        bytes.extend_from_slice(&(-480_i32).to_le_bytes());

        let found = describe(&bytes).expect("a bitmap");
        assert_eq!((found.width, found.height), (640, 480));
    }
}
