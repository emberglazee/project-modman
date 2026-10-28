//! Cooked `SoundWave` extraction.
//!
//! Project Wingman's audio is Ogg Vorbis, stored two ways:
//!
//! * **inline**: the whole stream sits in the `.uexp` (most SFX and dialog);
//! * **streamed**: a 256 KB prefix sits in the `.uexp` and the rest in a
//!   sibling `.ubulk` (the long music tracks, and any mod that imports audio
//!   with streaming enabled).
//!
//! Both begin at an Ogg BOS page whose first packet is the Vorbis
//! identification header. That is what makes a search for the stream start
//! safe: a name table can legitimately contain the text `OggS`, but it cannot
//! contain `\x01vorbis` behind a BOS page whose segment table says one 30-byte
//! packet.
//!
//! The end comes from the container as well. Walking the page chain to the page
//! that sets the EOS flag avoids trusting UE's size field, which for a streamed
//! asset describes only the inline chunk, and it drops the fixed trailer that
//! follows an inline stream.

use crate::export::ExportEntry;
use crate::header::PackageHeader;
use crate::names::read_name_table;
use crate::Error;
use std::io::{Cursor, Seek, SeekFrom};

/// The Ogg stream pulled out of one baked SoundWave.
pub struct ExtractedSound {
    /// The Vorbis stream, trailing padding removed.
    pub stream: Vec<u8>,
    /// True when a `.ubulk` sibling contributed bytes.
    pub streamed: bool,
    pub warnings: Vec<String>,
}

/// The engine class of the asset's first export, resolved through the import
/// table (`SoundWave`, `SoundCue`, `SoundAttenuation`, ...).
///
/// Returns `Ok(None)` when the class lives in another export, which no sound
/// asset does, rather than guessing.
pub fn export_class(uasset: &[u8]) -> Result<Option<String>, Error> {
    let mut cur = Cursor::new(uasset);
    let header = PackageHeader::read(&mut cur)?;
    if header.export_count <= 0 {
        return Ok(None);
    }

    let mut names_cur = Cursor::new(uasset);
    names_cur.seek(SeekFrom::Start(header.name_offset as u64))?;
    let names = read_name_table(&mut names_cur, header.name_count)?;

    let mut export_cur = Cursor::new(uasset);
    export_cur.seek(SeekFrom::Start(header.export_offset as u64))?;
    let export = ExportEntry::read(&mut export_cur)?;

    let class_index = export.class_index.index;
    if class_index >= 0 {
        return Ok(None);
    }
    let import_index = (-class_index) as usize - 1;
    if import_index >= header.import_count as usize {
        return Ok(None);
    }
    // Import entries are 28 bytes: ClassPackage, ClassName, OuterIndex,
    // ObjectName; the last field is the name that identifies the class.
    let object_name_off = header.import_offset as usize + import_index * 28 + 20;
    if object_name_off + 4 > uasset.len() {
        return Ok(None);
    }
    let name_index = i32::from_le_bytes(
        uasset[object_name_off..object_name_off + 4]
            .try_into()
            .unwrap(),
    );
    if name_index < 0 {
        return Ok(None);
    }
    Ok(names.get(name_index as usize).cloned())
}

/// Pull the Vorbis stream out of a cooked SoundWave pair.
///
/// `uexp` is the export payload and `ubulk` the sibling bulk file when the pak
/// ships one. The caller is expected to have confirmed the class is
/// `SoundWave`; a SoundCue has no payload and yields an error here.
pub fn extract(uexp: &[u8], ubulk: Option<&[u8]>) -> Result<ExtractedSound, Error> {
    let mut warnings = Vec::new();

    // Most sounds start their stream inline. A fully streamed asset can keep the
    // whole thing in the bulk, so fall back to looking there before giving up.
    let (start, source): (usize, &[u8]) = match find_vorbis_bos(uexp) {
        Some(p) => (p, uexp),
        None => match ubulk.and_then(find_vorbis_bos) {
            Some(p) => {
                warnings.push("stream starts in the .ubulk (no inline chunk)".to_string());
                (p, ubulk.unwrap())
            }
            None => {
                let flags = uexp
                    .get(0..4)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                    .unwrap_or(0);
                if flags & 0x02 != 0 {
                    return Err(Error::Parse(format!(
                        "payload is UE-compressed (bulk flags 0x{flags:02X}); this build cannot decompress it"
                    )));
                }
                return Err(Error::Parse("no Ogg Vorbis stream in payload".to_string()));
            }
        },
    };

    // Read the inline chunk from whichever payload holds the stream start.
    let inline = if std::ptr::eq(source, uexp) {
        inline_len(uexp, start).unwrap_or(uexp.len() - start)
    } else {
        source.len() - start
    };
    let mut stream = source[start..start + inline].to_vec();

    let streamed = ubulk.is_some_and(|b| !b.is_empty());
    if std::ptr::eq(source, uexp) {
        if let Some(bulk) = ubulk {
            stream.extend_from_slice(bulk);
        }
    }

    match stream_end(&stream) {
        Some(end) if end < stream.len() => {
            warnings.push(format!(
                "{} trailing bytes after the Ogg EOS page dropped",
                stream.len() - end
            ));
            stream.truncate(end);
        }
        Some(_) => {}
        None => warnings.push("no Ogg EOS page found; kept every byte".to_string()),
    }

    Ok(ExtractedSound {
        stream,
        streamed,
        warnings,
    })
}

/// First Ogg BOS page carrying a Vorbis identification packet.
fn find_vorbis_bos(d: &[u8]) -> Option<usize> {
    let needle = b"OggS";
    let mut from = 0usize;
    while from + 4 <= d.len() {
        let rel = d[from..].windows(4).position(|w| w == needle)?;
        let p = from + rel;
        if p + 35 <= d.len()
            && d[p + 4] == 0                 // stream_structure_version
            && d[p + 5] & 0x02 != 0          // BOS
            && d[p + 26] >= 1                // at least one segment
            && d[p + 27] == 30               // its length: the 30-byte ID header
            && d[p + 28] == 0x01
            && &d[p + 29..p + 35] == b"vorbis"
        {
            return Some(p);
        }
        from = p + 1;
    }
    None
}

/// The repeated length field in front of the stream, if it is present.
fn inline_len(d: &[u8], start: usize) -> Option<usize> {
    if start < 16 {
        return None;
    }
    let a = i32::from_le_bytes(d[start - 16..start - 12].try_into().ok()?);
    let b = i32::from_le_bytes(d[start - 12..start - 8].try_into().ok()?);
    if a == b && a > 0 && (a as usize) <= d.len() - start {
        Some(a as usize)
    } else {
        None
    }
}

/// End offset and header-type flags of the Ogg page starting at `p`.
fn page_end(d: &[u8], p: usize) -> Option<(usize, u8)> {
    if p + 27 > d.len() {
        return None;
    }
    let segments = d[p + 26] as usize;
    if p + 27 + segments > d.len() {
        return None;
    }
    let body: usize = d[p + 27..p + 27 + segments]
        .iter()
        .map(|&n| n as usize)
        .sum();
    let end = p + 27 + segments + body;
    if end > d.len() {
        None
    } else {
        Some((end, d[p + 5]))
    }
}

/// Offset just past the page that sets EOS, or `None` if the chain ends first.
fn stream_end(d: &[u8]) -> Option<usize> {
    let mut p = 0usize;
    for _ in 0..1_000_000 {
        let (end, flags) = page_end(d, p)?;
        if flags & 0x04 != 0 {
            return Some(end);
        }
        if end <= p {
            return None;
        }
        p = end;
        if p >= d.len() {
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> Vec<u8> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name);
        std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    #[test]
    fn dialog_line_extracts_to_a_complete_stream() {
        let uasset = fixture("sound_dialog.uasset");
        let uexp = fixture("sound_dialog.uexp");

        assert_eq!(export_class(&uasset).unwrap().as_deref(), Some("SoundWave"));

        let out = extract(&uexp, None).unwrap();
        assert!(out.stream.starts_with(b"OggS"));
        assert_eq!(out.stream[4], 0, "stream_structure_version");
        assert_ne!(out.stream[5] & 0x02, 0, "BOS flag");
        assert_eq!(
            &out.stream[29..35],
            b"vorbis",
            "Vorbis identification header"
        );
        assert_eq!(out.stream.len(), 50504, "engine length field");
        assert_eq!(
            stream_end(&out.stream),
            Some(out.stream.len()),
            "ends at EOS"
        );
        assert!(!out.streamed, "dialog ships inline");
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    }

    #[test]
    fn sound_cue_has_no_audio_payload() {
        let uasset = fixture("sound_cue.uasset");
        let uexp = fixture("sound_cue.uexp");
        assert_eq!(export_class(&uasset).unwrap().as_deref(), Some("SoundCue"));
        assert!(extract(&uexp, None).is_err(), "a cue carries no stream");
    }

    #[test]
    fn inline_length_is_read_from_the_repeated_field() {
        let uexp = fixture("sound_dialog.uexp");
        let start = find_vorbis_bos(&uexp).expect("BOS page");
        assert_eq!(inline_len(&uexp, start), Some(50504));
    }

    /// Streaming assets split at 256 KB with the rest in a `.ubulk`. The corpus
    /// is not in the repo, so this skips when the sample directory is absent.
    #[test]
    fn streamed_track_reassembles_from_uexp_plus_ubulk() {
        // The streamed sample is external; point MODMAN_TEST_CORPUS at the
        // folder that holds pw-audio-sample/ to run this test.
        let Some(root) = std::env::var_os("MODMAN_TEST_CORPUS").map(PathBuf::from) else {
            eprintln!("skipping: MODMAN_TEST_CORPUS is not set");
            return;
        };
        let dir = root.join("pw-audio-sample/PW_-_Mission_1.uexp");
        if !dir.is_file() {
            eprintln!("skipping: sample corpus not present");
            return;
        }
        let uexp = std::fs::read(&dir).unwrap();
        let ubulk = std::fs::read(dir.with_extension("ubulk")).unwrap();

        let out = extract(&uexp, Some(&ubulk)).unwrap();
        assert!(out.streamed);
        assert!(out.stream.starts_with(b"OggS"));
        assert_ne!(out.stream[5] & 0x02, 0, "BOS flag");
        // Ends exactly at the EOS page: the bulk's padding is gone.
        assert_eq!(stream_end(&out.stream), Some(out.stream.len()));
        // The inline chunk alone cannot carry this stream.
        let start = find_vorbis_bos(&uexp).unwrap();
        let inline = inline_len(&uexp, start).unwrap();
        assert!(out.stream.len() > inline);
    }
}
