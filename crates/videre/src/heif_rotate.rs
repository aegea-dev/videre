//! Turning a HEIC a quarter turn in place.
//!
//! A HEIC's displayed rotation is the HEIF `irot` item property, which
//! QuickLook applies; the EXIF Orientation tag beside it is only a copy for
//! applications that read EXIF. So a turn changes the angle byte of every
//! `irot` box that belongs to the primary image or to an item linked to it
//! (an iPhone's thumbnail and depth or gain-map images share the primary's
//! box), and the matching Orientation value. Both edits are the same size as
//! what they replace: no box grows, no `iloc` offset moves, and no content
//! byte changes, so the content key stays and only `meta_hash` changes.
//!
//! A file whose primary has no `irot` (an older iPhone wrote only the EXIF
//! tag, which QuickLook ignores on a HEIC, so the photo shows as stored) is
//! given one: an `irot` box appended to `ipco`, associated with the primary
//! and its thumbnails in `ipma`. That grows `meta` and its boxes, so every
//! `iloc` offset into the file past `meta` moves by the same amount. No item's
//! data and no `hvcC` changes, so the content key stays; the result is read
//! back and its content key compared before it is returned. What cannot be
//! rewritten safely (an association index too large for `ipma`, an offset
//! too large for its field, data stored another way) is refused.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Seek};
use std::ops::Range;

use crate::bmff::{self, read_at};

/// Why a HEIC cannot be turned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The file does not parse as HEIF.
    Malformed,
    /// The primary has no `irot`, and one cannot be added safely: why.
    CannotInsert(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::CannotInsert(why) => {
                write!(
                    f,
                    "this HEIC has no rotation property and one cannot be added: {why}"
                )
            }
            Error::Malformed => write!(f, "this file does not parse as HEIC"),
        }
    }
}

impl std::error::Error for Error {}

/// A HEIC's rotation and the bytes a turn rewrites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeifRotation {
    /// The primary image's `irot`: counter-clockwise quarter turns, 0 to 3.
    pub angle: u8,
    /// The primary image's size as displayed: `ispe` with `angle` applied.
    pub display: (u32, u32),
    /// File offsets of the angle byte of every `irot` a turn changes, each
    /// once, however many items share it.
    pub irot_at: Vec<u64>,
    /// File offset of the EXIF Orientation value, and whether its TIFF data
    /// is big-endian. `None` when there is no Orientation tag to keep in step.
    pub orientation_at: Option<(u64, bool)>,
}

/// The EXIF Orientation that displays like an `irot` of `angle`.
pub fn orientation_for(angle: u8) -> u16 {
    match angle & 3 {
        0 => 1,
        1 => 8,
        2 => 3,
        _ => 6,
    }
}

/// `angle` turned a quarter clockwise, or counter-clockwise when `ccw`.
/// `irot` counts counter-clockwise, so clockwise is three steps on.
pub fn turned(angle: u8, ccw: bool) -> u8 {
    if ccw {
        (angle + 1) % 4
    } else {
        (angle + 3) % 4
    }
}

/// Read `bytes` as HEIF: the primary's angle, its displayed size, and where a
/// turn writes.
pub fn read(bytes: &[u8]) -> Result<HeifRotation, Error> {
    parse(bytes).ok_or(Error::Malformed)?
}

/// `bytes` turned a quarter clockwise, or counter-clockwise when `ccw`, as a
/// same-length copy. Each `irot` turns by one step from its own angle, so an
/// item stored at a different angle from the primary keeps its difference.
pub fn rotated(bytes: &[u8], ccw: bool) -> Result<Vec<u8>, Error> {
    let r = read(bytes)?;
    if r.irot_at.is_empty() {
        return inserted(bytes, &r, turned(0, ccw));
    }
    let mut out = bytes.to_vec();
    for &at in &r.irot_at {
        let b = &mut out[at as usize];
        *b = (*b & !3) | turned(*b & 3, ccw);
    }
    if let Some((at, big)) = r.orientation_at {
        let value = orientation_for(turned(r.angle, ccw));
        let v = if big {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        };
        out[at as usize..at as usize + 2].copy_from_slice(&v);
    }
    Ok(out)
}

/// The primary's angle and displayed size, and the `irot` boxes a turn
/// changes, from the file's boxes alone: no item data is read.
struct Layout {
    children: Vec<bmff::Bmff>,
    angle: u8,
    display: (u32, u32),
    irot_at: Vec<u64>,
}

fn layout<F: Read + Seek>(f: &mut F, len: u64) -> Option<Layout> {
    let children = bmff::meta_children(f, len)?;
    let primary = primary_item(f, &children, len)?;
    let related = related_items(f, &children, primary, len)?;

    // Properties are numbered from 1 in ipco order.
    let iprp = bmff::find(&children, b"iprp")?;
    let iprp_children = bmff::boxes(f, iprp.payload..iprp.end, len)?;
    let ipco = bmff::find(&iprp_children, b"ipco")?;
    let properties = bmff::boxes(f, ipco.payload..ipco.end, len)?;
    let associations = associations(f, &iprp_children, len)?;
    let property_of = |item: u32, kind: &[u8; 4]| -> Option<&bmff::Bmff> {
        associations
            .get(&item)?
            .iter()
            .filter_map(|&i| properties.get(i.checked_sub(1)? as usize))
            .find(|p| &p.kind == kind)
    };

    let angle = match property_of(primary, b"irot") {
        Some(irot) if irot.payload >= irot.end => return None,
        Some(irot) => read_at(f, irot.payload, 1, len)?[0] & 3,
        None => 0,
    };
    let irot_at: BTreeSet<u64> = related
        .iter()
        .filter_map(|&item| property_of(item, b"irot"))
        .filter(|p| p.payload < p.end)
        .map(|p| p.payload)
        .collect();

    // ispe is a full box: version and flags, then width and height.
    let ispe = property_of(primary, b"ispe")?;
    let wh = read_at(f, ispe.payload + 4, 8, len)?;
    let (w, h) = (
        u32::from_be_bytes(wh[0..4].try_into().ok()?),
        u32::from_be_bytes(wh[4..8].try_into().ok()?),
    );
    let display = if angle % 2 == 1 { (h, w) } else { (w, h) };
    Some(Layout {
        children,
        angle,
        display,
        irot_at: irot_at.into_iter().collect(),
    })
}

/// The size a HEIC is shown at, its primary's `ispe` turned by its `irot`,
/// read from the boxes at the start of the file without its image data.
pub fn displayed_size<F: Read + Seek>(f: &mut F, len: u64) -> Option<(u32, u32)> {
    layout(f, len).map(|l| l.display)
}

/// `None` for a file that does not parse. A primary with no `irot` reads as
/// angle 0 with nothing in `irot_at`: shown as stored.
fn parse(bytes: &[u8]) -> Option<Result<HeifRotation, Error>> {
    let len = bytes.len() as u64;
    let f = &mut Cursor::new(bytes);
    let l = layout(f, len)?;
    let types = bmff::item_types(f, &l.children, len)?;
    let extents = bmff::item_extents(f, &l.children, len)?;
    let orientation_at = types
        .iter()
        .find(|(_, kind)| *kind == b"Exif")
        .and_then(|(id, _)| extents.get(id))
        .and_then(|ranges| exif_orientation(bytes, ranges));

    Some(Ok(HeifRotation {
        angle: l.angle,
        display: l.display,
        irot_at: l.irot_at,
        orientation_at,
    }))
}

/// The primary item's ID, from `pitm`.
fn primary_item<F: Read + Seek>(f: &mut F, children: &[bmff::Bmff], len: u64) -> Option<u32> {
    let pitm = bmff::find(children, b"pitm")?;
    let head = read_at(f, pitm.payload, 8.min(pitm.end - pitm.payload), len)?;
    Some(match head.first()? {
        0 => u16::from_be_bytes(head.get(4..6)?.try_into().ok()?) as u32,
        _ => u32::from_be_bytes(head.get(4..8)?.try_into().ok()?),
    })
}

/// The items that are thumbnails of `primary` (`thmb` references to it): the
/// ones an iPhone shares its `irot` with.
fn thumbnails<F: Read + Seek>(
    f: &mut F,
    children: &[bmff::Bmff],
    primary: u32,
    len: u64,
) -> Option<Vec<u32>> {
    let mut out = Vec::new();
    let Some(iref) = bmff::find(children, b"iref") else {
        return Some(out);
    };
    let version = read_at(f, iref.payload, 1, len)?[0];
    let id_size = if version == 0 { 2 } else { 4 };
    for reference in bmff::boxes(f, iref.payload + 4..iref.end, len)? {
        if &reference.kind != b"thmb" {
            continue;
        }
        let body = read_at(f, reference.payload, reference.end - reference.payload, len)?;
        let id = |at: usize| -> Option<u32> {
            let b = body.get(at..at + id_size)?;
            Some(b.iter().fold(0u32, |acc, x| (acc << 8) | *x as u32))
        };
        let from = id(0)?;
        let count = u16::from_be_bytes(body.get(id_size..id_size + 2)?.try_into().ok()?) as usize;
        for i in 0..count {
            if id(id_size + 2 + i * id_size)? == primary {
                out.push(from);
            }
        }
    }
    Some(out)
}

/// The bytes of an essential association with property `index`, or `None`
/// when `index` does not fit the width `ipma` uses.
fn association(index: usize, wide: bool) -> Option<Vec<u8>> {
    match (wide, index) {
        (false, 1..=0x7F) => Some(vec![0x80 | index as u8]),
        (true, 1..=0x7FFF) => Some((0x8000 | index as u16).to_be_bytes().to_vec()),
        _ => None,
    }
}

/// An `iloc` number moved by `by`, or `None` when the result does not fit its
/// field (a field of size 0 is absent and cannot hold a new value).
fn moved(field: &bmff::IlocField, by: u64) -> Option<u64> {
    let value = field.value.checked_add(by)?;
    match field.size {
        0 => None,
        8 => Some(value),
        n => (value < 1u64 << (8 * n)).then_some(value),
    }
}

/// Write `value` big-endian into the `size` bytes at `at`.
fn put(out: &mut [u8], at: u64, size: usize, value: u64) {
    let at = at as usize;
    let all = value.to_be_bytes();
    out[at..at + size].copy_from_slice(&all[8 - size..]);
}

/// A box's size field rewritten for `grew` more bytes. A size of 0 runs to the
/// end of its parent and is left alone.
fn grow_box(bytes: &[u8], out: &mut [u8], b: &bmff::Bmff, grew: u64) -> Result<(), Error> {
    let at = b.start as usize;
    let size = u32::from_be_bytes(bytes[at..at + 4].try_into().map_err(|_| Error::Malformed)?);
    match size {
        0 => Ok(()),
        1 => {
            let total = (b.end - b.start) + grew;
            put(out, b.start + 8, 8, total);
            Ok(())
        }
        _ => {
            let total = u32::try_from((b.end - b.start) + grew)
                .map_err(|_| Error::CannotInsert("a box would outgrow its size field"))?;
            put(out, b.start, 4, total as u64);
            Ok(())
        }
    }
}

/// `bytes` given an `irot` of `angle` on the primary and its thumbnails, with
/// the EXIF Orientation set to match. Read back before it is returned: the
/// angle must be the one asked for and the content key unchanged.
fn inserted(bytes: &[u8], r: &HeifRotation, angle: u8) -> Result<Vec<u8>, Error> {
    let len = bytes.len() as u64;
    let f = &mut Cursor::new(bytes);
    let m = Error::Malformed;
    let top = bmff::boxes(f, 0..len, len).ok_or(m.clone())?;
    let meta = top.iter().find(|b| &b.kind == b"meta").ok_or(m.clone())?;
    let children = bmff::boxes(f, meta.payload + 4..meta.end, len).ok_or(m.clone())?;
    let primary = primary_item(f, &children, len).ok_or(m.clone())?;
    let mut targets = vec![primary];
    targets.extend(thumbnails(f, &children, primary, len).ok_or(m.clone())?);

    let iprp = bmff::find(&children, b"iprp").ok_or(m.clone())?;
    let iprp_children = bmff::boxes(f, iprp.payload..iprp.end, len).ok_or(m.clone())?;
    let ipco = bmff::find(&iprp_children, b"ipco").ok_or(m.clone())?;
    let properties = bmff::boxes(f, ipco.payload..ipco.end, len).ok_or(m.clone())?;
    let ipmas: Vec<&bmff::Bmff> = iprp_children
        .iter()
        .filter(|b| &b.kind == b"ipma")
        .collect();
    if ipmas.len() != 1 {
        return Err(Error::CannotInsert(
            "not exactly one property association box",
        ));
    }
    let ipma = ipmas[0];

    // The new property's index, and where each target's association goes.
    let body = read_at(f, ipma.payload, ipma.end - ipma.payload, len).ok_or(m.clone())?;
    let version = *body.first().ok_or(m.clone())?;
    let wide = body.get(3).ok_or(m.clone())? & 1 == 1;
    let new = association(properties.len() + 1, wide).ok_or(Error::CannotInsert(
        "too many properties for the association table",
    ))?;
    let mut at = 4usize;
    let take = |at: &mut usize, n: usize| -> Option<u32> {
        let b = body.get(*at..*at + n)?;
        *at += n;
        Some(b.iter().fold(0u32, |acc, x| (acc << 8) | *x as u32))
    };
    let count = take(&mut at, 4).ok_or(m.clone())?;
    // (position of the count byte, its value, end of the entry) per target.
    let mut entries: Vec<(u64, u8, u64)> = Vec::new();
    for _ in 0..count {
        let item = take(&mut at, if version < 1 { 2 } else { 4 }).ok_or(m.clone())?;
        let count_at = at;
        let n = take(&mut at, 1).ok_or(m.clone())? as u8;
        at += n as usize * if wide { 2 } else { 1 };
        if at > body.len() {
            return Err(m);
        }
        if targets.contains(&item) {
            entries.push((ipma.payload + count_at as u64, n, ipma.payload + at as u64));
        }
    }
    if entries.len() != targets.len() {
        return Err(Error::CannotInsert(
            "an image has no property associations to extend",
        ));
    }
    if entries.iter().any(|&(_, n, _)| n == u8::MAX) {
        return Err(Error::CannotInsert(
            "an image has too many property associations",
        ));
    }

    let irot = [0, 0, 0, 9, b'i', b'r', b'o', b't', angle & 3];
    let ipma_grew = (new.len() * entries.len()) as u64;
    let grew = irot.len() as u64 + ipma_grew;

    // Every number is rewritten in place first, at its original position;
    // then the new bytes go in from the back, so no earlier position moves.
    let mut out = bytes.to_vec();
    grow_box(bytes, &mut out, ipco, irot.len() as u64)?;
    grow_box(bytes, &mut out, ipma, ipma_grew)?;
    grow_box(bytes, &mut out, iprp, grew)?;
    grow_box(bytes, &mut out, meta, grew)?;
    for &(count_at, n, _) in &entries {
        out[count_at as usize] = n + 1;
    }
    for item in bmff::iloc_items(f, &children, len).ok_or(m.clone())? {
        // Only data addressed by file offset moves, and only past meta.
        if item.method != 0 {
            continue;
        }
        let starts: Vec<u64> = item
            .extents
            .iter()
            .map(|(o, _)| item.base.value.saturating_add(o.value))
            .collect();
        let after = starts.iter().filter(|&&s| s >= meta.end).count();
        if after == 0 {
            continue;
        }
        let refuse = Error::CannotInsert("an item offset does not fit its field");
        if item.base.size > 0 {
            if after != starts.len() {
                return Err(Error::CannotInsert(
                    "an item's data lies both sides of meta",
                ));
            }
            let v = moved(&item.base, grew).ok_or(refuse)?;
            put(&mut out, item.base.at, item.base.size, v);
        } else {
            for ((offset, _), start) in item.extents.iter().zip(&starts) {
                if *start >= meta.end {
                    let v = moved(offset, grew).ok_or(refuse.clone())?;
                    put(&mut out, offset.at, offset.size, v);
                }
            }
        }
    }
    if let Some((at, big)) = r.orientation_at {
        let value = orientation_for(angle);
        let v = if big {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        };
        out[at as usize..at as usize + 2].copy_from_slice(&v);
    }

    let mut inserts: Vec<(u64, Vec<u8>)> = entries
        .iter()
        .map(|&(_, _, end)| (end, new.clone()))
        .collect();
    inserts.push((ipco.end, irot.to_vec()));
    inserts.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    for (at, data) in inserts {
        out.splice(at as usize..at as usize, data);
    }

    // Read it back: the angle asked for, and the same content.
    let back = read(&out)?;
    let keys = |b: &[u8]| {
        crate::content_key::keys(
            &mut Cursor::new(b),
            b.len() as u64,
            Some(crate::content_key::Format::Heic),
        )
    };
    let same_content = matches!(
        (keys(bytes), keys(&out)),
        (Ok(a), Ok(b)) if a.content == b.content
    );
    if back.angle != angle & 3 || back.irot_at.is_empty() || !same_content {
        return Err(Error::CannotInsert(
            "the rewritten file did not read back the same",
        ));
    }
    Ok(out)
}

/// The primary and every item linked to it through `iref`, in either
/// direction: its thumbnail, auxiliary images (depth, gain map) and, for a
/// grid, its tiles. The `Exif` item's `cdsc` link is harmless: it has no
/// `irot`.
fn related_items<F: Read + Seek>(
    f: &mut F,
    children: &[bmff::Bmff],
    primary: u32,
    len: u64,
) -> Option<BTreeSet<u32>> {
    let mut related = BTreeSet::from([primary]);
    let Some(iref) = bmff::find(children, b"iref") else {
        return Some(related);
    };
    let version = read_at(f, iref.payload, 1, len)?[0];
    let id_size = if version == 0 { 2 } else { 4 };
    for reference in bmff::boxes(f, iref.payload + 4..iref.end, len)? {
        let body = read_at(f, reference.payload, reference.end - reference.payload, len)?;
        let id = |at: usize| -> Option<u32> {
            let b = body.get(at..at + id_size)?;
            Some(b.iter().fold(0u32, |acc, x| (acc << 8) | *x as u32))
        };
        let from = id(0)?;
        let count = u16::from_be_bytes(body.get(id_size..id_size + 2)?.try_into().ok()?) as usize;
        let to: Vec<u32> = (0..count)
            .map(|i| id(id_size + 2 + i * id_size))
            .collect::<Option<_>>()?;
        if from == primary {
            related.extend(&to);
        } else if to.contains(&primary) {
            related.insert(from);
        }
    }
    Some(related)
}

/// Every item's property indices from `ipma`.
fn associations<F: Read + Seek>(
    f: &mut F,
    iprp_children: &[bmff::Bmff],
    len: u64,
) -> Option<BTreeMap<u32, Vec<u16>>> {
    let mut all: BTreeMap<u32, Vec<u16>> = BTreeMap::new();
    for ipma in iprp_children.iter().filter(|b| &b.kind == b"ipma") {
        let body = read_at(f, ipma.payload, ipma.end - ipma.payload, len)?;
        let version = *body.first()?;
        let wide = body.get(3)? & 1 == 1;
        let mut at = 4usize;
        let mut take = |n: usize| -> Option<u32> {
            let b = body.get(at..at + n)?;
            at += n;
            Some(b.iter().fold(0u32, |acc, x| (acc << 8) | *x as u32))
        };
        let count = take(4)?;
        for _ in 0..count {
            let item = take(if version < 1 { 2 } else { 4 })?;
            let n = take(1)?;
            let list = all.entry(item).or_default();
            for _ in 0..n {
                let index = if wide {
                    take(2)? & 0x7FFF
                } else {
                    take(1)? & 0x7F
                };
                list.push(index as u16);
            }
        }
    }
    Some(all)
}

/// Where the Orientation value sits in an `Exif` item, as a file offset, and
/// whether the TIFF data is big-endian. The item's data is a 4-byte offset to
/// the TIFF header, then TIFF; only IFD0 is searched, where the tag lives.
fn exif_orientation(bytes: &[u8], ranges: &[Range<u64>]) -> Option<(u64, bool)> {
    // The item's data may be split across extents: address it as one run.
    let data: Vec<u8> = ranges
        .iter()
        .flat_map(|r| bytes.get(r.start as usize..r.end as usize).unwrap_or(&[]))
        .copied()
        .collect();
    let to_file = |mut at: usize| -> Option<u64> {
        for r in ranges {
            let n = (r.end - r.start) as usize;
            if at < n {
                return Some(r.start + at as u64);
            }
            at -= n;
        }
        None
    };
    let tiff = 4 + u32::from_be_bytes(data.get(0..4)?.try_into().ok()?) as usize;
    let big = match data.get(tiff..tiff + 2)? {
        b"MM" => true,
        b"II" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let b: [u8; 2] = data.get(at..at + 2)?.try_into().ok()?;
        Some(if big {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let b: [u8; 4] = data.get(at..at + 4)?.try_into().ok()?;
        Some(if big {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    };
    let ifd0 = tiff.checked_add(u32_at(tiff + 4)? as usize)?;
    let entries = u16_at(ifd0)? as usize;
    for i in 0..entries {
        let entry = ifd0 + 2 + i * 12;
        // Tag 0x0112, type SHORT, one value: stored inline at entry + 8.
        if u16_at(entry)? == 0x0112 {
            if u16_at(entry + 2)? != 3 || u32_at(entry + 4)? != 1 {
                return None;
            }
            return Some((to_file(entry + 8)?, big));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(rel: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/tests/fixtures/{rel}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    fn keys(bytes: &[u8]) -> crate::content_key::Keys {
        crate::content_key::keys(
            &mut Cursor::new(bytes),
            bytes.len() as u64,
            Some(crate::content_key::Format::Heic),
        )
        .unwrap()
    }

    /// The Orientation value a rotation wrote, read back through the offset.
    fn orientation(bytes: &[u8]) -> u16 {
        let (at, big) = read(bytes).unwrap().orientation_at.unwrap();
        let b = [bytes[at as usize], bytes[at as usize + 1]];
        if big {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        }
    }

    #[test]
    fn a_single_image_heic_has_one_irot_at_270() {
        let r = read(&fixture("content_key/tiny.heic")).unwrap();
        assert_eq!(r.angle, 3);
        assert_eq!(r.irot_at.len(), 1);
        // ispe is 16x12; a quarter turn displays it 12 wide.
        assert_eq!(r.display, (12, 16));
    }

    #[test]
    fn a_grid_and_its_thumbnail_share_one_irot() {
        let bytes = fixture("heic/grid_rot90.heic");
        let r = read(&bytes).unwrap();
        assert_eq!(r.angle, 3);
        assert_eq!(r.irot_at.len(), 1, "one shared box, changed once");
        assert_eq!(r.display, (1536, 2048));
        assert_eq!(orientation(&bytes), 6);
    }

    /// The items each `irot` is associated with, by the box's angle.
    fn irot_items(bytes: &[u8]) -> Vec<(u32, u8)> {
        let len = bytes.len() as u64;
        let f = &mut Cursor::new(bytes);
        let children = bmff::meta_children(f, len).unwrap();
        let iprp = bmff::find(&children, b"iprp").unwrap();
        let iprp_children = bmff::boxes(f, iprp.payload..iprp.end, len).unwrap();
        let ipco = bmff::find(&iprp_children, b"ipco").unwrap();
        let properties = bmff::boxes(f, ipco.payload..ipco.end, len).unwrap();
        let mut out = Vec::new();
        for (item, indices) in associations(f, &iprp_children, len).unwrap() {
            for i in indices {
                let p = &properties[i as usize - 1];
                if &p.kind == b"irot" {
                    out.push((item, bytes[p.payload as usize] & 3));
                }
            }
        }
        out
    }

    fn primary_and_thumbnail(bytes: &[u8]) -> (u32, Vec<u32>) {
        let len = bytes.len() as u64;
        let f = &mut Cursor::new(bytes);
        let children = bmff::meta_children(f, len).unwrap();
        let primary = primary_item(f, &children, len).unwrap();
        (primary, thumbnails(f, &children, primary, len).unwrap())
    }

    #[test]
    fn a_heic_without_irot_reads_as_angle_zero() {
        let bytes = fixture("heic/grid_no_irot.heic");
        assert_eq!(bytes.windows(4).filter(|w| w == b"irot").count(), 0);
        let r = read(&bytes).unwrap();
        assert_eq!(r.angle, 0);
        assert!(r.irot_at.is_empty());
        assert_eq!(r.display, (2048, 1536), "shown as stored");
        assert_eq!(orientation(&bytes), 6, "the wrong tag QuickLook ignores");
    }

    #[test]
    fn a_turn_inserts_one_irot_for_the_primary_and_its_thumbnail() {
        let original = fixture("heic/grid_no_irot.heic");
        let turned_bytes = rotated(&original, true).unwrap();
        assert_eq!(turned_bytes.windows(4).filter(|w| w == b"irot").count(), 1);
        let r = read(&turned_bytes).unwrap();
        assert_eq!(r.angle, 1, "a quarter counter-clockwise");
        assert_eq!(r.display, (1536, 2048));
        let (primary, thumbs) = primary_and_thumbnail(&turned_bytes);
        assert_eq!(thumbs.len(), 1);
        let mut expected = vec![(primary, 1), (thumbs[0], 1)];
        expected.sort();
        let mut got = irot_items(&turned_bytes);
        got.sort();
        assert_eq!(got, expected, "the tiles are not turned on their own");
    }

    #[test]
    fn an_insertion_grows_meta_and_nothing_beside_it() {
        let original = fixture("heic/grid_no_irot.heic");
        let turned_bytes = rotated(&original, false).unwrap();
        let top = |b: &[u8]| {
            bmff::boxes(&mut Cursor::new(b), 0..b.len() as u64, b.len() as u64)
                .unwrap()
                .into_iter()
                .map(|x| (x.kind, x.end - x.start))
                .collect::<Vec<_>>()
        };
        let (before, after) = (top(&original), top(&turned_bytes));
        let grew = (turned_bytes.len() - original.len()) as u64;
        assert!(grew > 0);
        assert_eq!(before.len(), after.len());
        for (b, a) in before.iter().zip(&after) {
            assert_eq!(b.0, a.0);
            let expected = if &b.0 == b"meta" { b.1 + grew } else { b.1 };
            assert_eq!(a.1, expected, "{}", String::from_utf8_lossy(&b.0));
        }
    }

    #[test]
    fn an_insertion_moves_the_item_data_offsets_with_it() {
        let original = fixture("heic/grid_no_irot.heic");
        let turned_bytes = rotated(&original, false).unwrap();
        let extents = |b: &[u8]| {
            let len = b.len() as u64;
            let f = &mut Cursor::new(b);
            let children = bmff::meta_children(f, len).unwrap();
            bmff::item_extents(f, &children, len).unwrap()
        };
        let (before, after) = (extents(&original), extents(&turned_bytes));
        assert_eq!(
            before.keys().collect::<Vec<_>>(),
            after.keys().collect::<Vec<_>>()
        );
        for (id, ranges) in &before {
            for (b, a) in ranges.iter().zip(&after[id]) {
                assert_eq!(
                    original[b.start as usize..b.end as usize],
                    turned_bytes[a.start as usize..a.end as usize],
                    "item {id} still points at its own data"
                );
            }
        }
    }

    #[test]
    fn an_insertion_keeps_the_content_key() {
        let original = fixture("heic/grid_no_irot.heic");
        let turned_bytes = rotated(&original, true).unwrap();
        let (before, after) = (keys(&original), keys(&turned_bytes));
        assert!(
            before.meta.is_some(),
            "the fixture must parse, not fall back"
        );
        assert_eq!(before.content, after.content);
        assert_ne!(before.meta, after.meta);
    }

    #[test]
    fn an_insertion_sets_the_exif_tag_to_match() {
        let original = fixture("heic/grid_no_irot.heic");
        // The fixture's tag says 90 clockwise; a counter-clockwise turn is 8.
        assert_eq!(orientation(&rotated(&original, true).unwrap()), 8);
        assert_eq!(orientation(&rotated(&original, false).unwrap()), 6);
    }

    #[test]
    fn after_an_insertion_turns_are_same_size_again() {
        let inserted = rotated(&fixture("heic/grid_no_irot.heic"), true).unwrap();
        let mut bytes = inserted.clone();
        for _ in 0..4 {
            bytes = rotated(&bytes, true).unwrap();
            assert_eq!(bytes.len(), inserted.len());
        }
        assert_eq!(bytes, inserted);
    }

    #[test]
    fn a_heic_whose_irot_is_unlinked_gets_a_new_one() {
        // Rename the irot property so the primary has none, as in a file
        // written without one; a turn then adds a real irot.
        let mut bytes = fixture("content_key/tiny.heic");
        let at = bytes.windows(4).position(|w| w == b"irot").unwrap();
        bytes[at..at + 4].copy_from_slice(b"frot");
        assert_eq!(read(&bytes).unwrap().angle, 0);
        let turned_bytes = rotated(&bytes, false).unwrap();
        assert_eq!(read(&turned_bytes).unwrap().angle, 3);
        assert_eq!(keys(&bytes).content, keys(&turned_bytes).content);
    }

    #[test]
    fn an_association_that_does_not_fit_is_refused() {
        assert_eq!(association(127, false), Some(vec![0x80 | 127]));
        assert_eq!(association(128, false), None);
        assert_eq!(association(128, true), Some(vec![0x80, 128]));
        assert_eq!(association(0x8000, true), None);
    }

    #[test]
    fn an_offset_that_does_not_fit_its_field_is_refused() {
        let field = |size| bmff::IlocField {
            at: 0,
            size,
            value: 0xFFFF_FFF0,
        };
        assert_eq!(moved(&field(4), 0x0F), Some(0xFFFF_FFFF));
        assert_eq!(moved(&field(4), 0x10), None);
        assert_eq!(moved(&field(8), 0x10), Some(0x1_0000_0000));
        assert_eq!(moved(&field(0), 1), None, "an absent field cannot move");
    }

    #[test]
    fn damaged_input_is_an_error_not_a_panic() {
        let bytes = fixture("heic/grid_rot90.heic");
        for cut in [0, 8, 40, 200, 600, bytes.len() / 2] {
            assert!(read(&bytes[..cut]).is_err(), "truncated at {cut}");
        }
        let noise: Vec<u8> = (0..4096u32)
            .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
            .collect();
        assert!(read(&noise).is_err());
    }

    #[test]
    fn four_clockwise_turns_give_back_the_same_bytes() {
        let original = fixture("heic/grid_rot90.heic");
        let mut bytes = original.clone();
        for _ in 0..4 {
            bytes = rotated(&bytes, false).unwrap();
            assert_eq!(bytes.len(), original.len(), "no box may grow");
        }
        assert_eq!(bytes, original);
    }

    #[test]
    fn a_clockwise_then_counter_clockwise_turn_is_no_change() {
        let original = fixture("heic/grid_rot90.heic");
        let back = rotated(&rotated(&original, false).unwrap(), true).unwrap();
        assert_eq!(back, original);
    }

    #[test]
    fn a_clockwise_turn_updates_irot_and_exif_together() {
        let turned_bytes = rotated(&fixture("heic/grid_rot90.heic"), false).unwrap();
        let r = read(&turned_bytes).unwrap();
        assert_eq!(r.angle, 2);
        assert_eq!(r.display, (2048, 1536));
        assert_eq!(orientation(&turned_bytes), 3);
    }

    #[test]
    fn a_turn_keeps_the_content_key_and_changes_the_metadata_hash() {
        let original = fixture("heic/grid_rot90.heic");
        let turned_bytes = rotated(&original, false).unwrap();
        let (before, after) = (keys(&original), keys(&turned_bytes));
        assert!(
            before.meta.is_some(),
            "the fixture must parse, not fall back"
        );
        assert_eq!(before.content, after.content);
        assert_ne!(before.meta, after.meta);
    }

    #[test]
    fn orientation_follows_the_angle() {
        assert_eq!(
            (0..4).map(orientation_for).collect::<Vec<_>>(),
            vec![1, 8, 3, 6]
        );
        assert_eq!(turned(3, false), 2);
        assert_eq!(turned(0, false), 3);
        assert_eq!(turned(3, true), 0);
    }
}
