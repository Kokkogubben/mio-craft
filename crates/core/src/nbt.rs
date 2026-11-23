//! Java Edition NBT: Minecraft's binary tag format (big-endian, strings in
//! "modified UTF-8"). Enough to read and rewrite small files such as
//! `servers.dat` while keeping every tag the launcher doesn't know about.

use anyhow::{bail, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    /// The element type id, then the elements (all of that type).
    List(u8, Vec<Tag>),
    /// Kept in file order, so rewriting a file changes as little as possible.
    Compound(Vec<(String, Tag)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Tag {
    pub fn id(&self) -> u8 {
        match self {
            Tag::Byte(_) => 1,
            Tag::Short(_) => 2,
            Tag::Int(_) => 3,
            Tag::Long(_) => 4,
            Tag::Float(_) => 5,
            Tag::Double(_) => 6,
            Tag::ByteArray(_) => 7,
            Tag::String(_) => 8,
            Tag::List(..) => 9,
            Tag::Compound(_) => 10,
            Tag::IntArray(_) => 11,
            Tag::LongArray(_) => 12,
        }
    }

    /// A child of a compound.
    pub fn get(&self, key: &str) -> Option<&Tag> {
        match self {
            Tag::Compound(c) => c.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Tag> {
        match self {
            Tag::Compound(c) => c.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Set a child of a compound, replacing one with the same name in place.
    pub fn set(&mut self, key: &str, value: Tag) {
        if let Tag::Compound(c) = self {
            match c.iter_mut().find(|(k, _)| k == key) {
                Some((_, v)) => *v = value,
                None => c.push((key.to_string(), value)),
            }
        }
    }

    pub fn remove(&mut self, key: &str) {
        if let Tag::Compound(c) = self {
            c.retain(|(k, _)| k != key);
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Tag::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Tag::Byte(v) => Some(v as i64),
            Tag::Short(v) => Some(v as i64),
            Tag::Int(v) => Some(v as i64),
            Tag::Long(v) => Some(v),
            _ => None,
        }
    }
}

// ---------- reading ----------

/// The largest number of elements in one array or list we accept, so a broken
/// file can't make us allocate gigabytes.
const MAX_LEN: usize = 1 << 24;
const MAX_DEPTH: usize = 512;

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        if self.b.len() - self.pos < n {
            bail!("The NBT data ends too early");
        }
        let s = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn arr<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into().unwrap())
    }
    fn len(&mut self) -> Result<usize> {
        let n = i32::from_be_bytes(self.arr()?);
        if n < 0 || n as usize > MAX_LEN {
            bail!("An NBT length is out of range ({n})");
        }
        Ok(n as usize)
    }
    fn string(&mut self) -> Result<String> {
        let n = u16::from_be_bytes(self.arr()?) as usize;
        Ok(from_mutf8(self.take(n)?))
    }
    fn payload(&mut self, id: u8, depth: usize) -> Result<Tag> {
        if depth > MAX_DEPTH {
            bail!("The NBT data is nested too deeply");
        }
        Ok(match id {
            1 => Tag::Byte(self.u8()? as i8),
            2 => Tag::Short(i16::from_be_bytes(self.arr()?)),
            3 => Tag::Int(i32::from_be_bytes(self.arr()?)),
            4 => Tag::Long(i64::from_be_bytes(self.arr()?)),
            5 => Tag::Float(f32::from_be_bytes(self.arr()?)),
            6 => Tag::Double(f64::from_be_bytes(self.arr()?)),
            7 => {
                let n = self.len()?;
                Tag::ByteArray(self.take(n)?.iter().map(|b| *b as i8).collect())
            }
            8 => Tag::String(self.string()?),
            9 => {
                let el = self.u8()?;
                let n = self.len()?;
                if el == 0 && n > 0 {
                    bail!("An NBT list of nothing has elements");
                }
                let mut v = Vec::with_capacity(n.min(1024));
                for _ in 0..n {
                    v.push(self.payload(el, depth + 1)?);
                }
                Tag::List(el, v)
            }
            10 => {
                let mut c = vec![];
                loop {
                    let t = self.u8()?;
                    if t == 0 {
                        break;
                    }
                    let name = self.string()?;
                    c.push((name, self.payload(t, depth + 1)?));
                }
                Tag::Compound(c)
            }
            11 => {
                let n = self.len()?;
                (0..n).map(|_| self.arr().map(i32::from_be_bytes)).collect::<Result<_>>().map(Tag::IntArray)?
            }
            12 => {
                let n = self.len()?;
                (0..n).map(|_| self.arr().map(i64::from_be_bytes)).collect::<Result<_>>().map(Tag::LongArray)?
            }
            _ => bail!("Unknown NBT tag type {id}"),
        })
    }
}

/// Read an uncompressed NBT file: one named root tag (normally a compound).
pub fn read(bytes: &[u8]) -> Result<Tag> {
    let mut r = Reader { b: bytes, pos: 0 };
    let id = r.u8()?;
    if id == 0 {
        bail!("The NBT file is empty");
    }
    let _name = r.string()?;
    r.payload(id, 0)
}

// ---------- writing ----------

fn write_string(out: &mut Vec<u8>, s: &str) -> Result<()> {
    let b = to_mutf8(s);
    if b.len() > u16::MAX as usize {
        bail!("A text in the NBT file is too long");
    }
    out.extend((b.len() as u16).to_be_bytes());
    out.extend(b);
    Ok(())
}

fn write_payload(out: &mut Vec<u8>, t: &Tag) -> Result<()> {
    match t {
        Tag::Byte(v) => out.push(*v as u8),
        Tag::Short(v) => out.extend(v.to_be_bytes()),
        Tag::Int(v) => out.extend(v.to_be_bytes()),
        Tag::Long(v) => out.extend(v.to_be_bytes()),
        Tag::Float(v) => out.extend(v.to_be_bytes()),
        Tag::Double(v) => out.extend(v.to_be_bytes()),
        Tag::ByteArray(v) => {
            out.extend((v.len() as i32).to_be_bytes());
            out.extend(v.iter().map(|b| *b as u8));
        }
        Tag::String(s) => write_string(out, s)?,
        Tag::List(el, v) => {
            let el = v.first().map(Tag::id).unwrap_or(*el);
            if v.iter().any(|x| x.id() != el) {
                bail!("An NBT list mixes tag types");
            }
            out.push(if v.is_empty() && el == 0 { 0 } else { el });
            out.extend((v.len() as i32).to_be_bytes());
            for x in v {
                write_payload(out, x)?;
            }
        }
        Tag::Compound(c) => {
            for (k, v) in c {
                out.push(v.id());
                write_string(out, k)?;
                write_payload(out, v)?;
            }
            out.push(0);
        }
        Tag::IntArray(v) => {
            out.extend((v.len() as i32).to_be_bytes());
            v.iter().for_each(|x| out.extend(x.to_be_bytes()));
        }
        Tag::LongArray(v) => {
            out.extend((v.len() as i32).to_be_bytes());
            v.iter().for_each(|x| out.extend(x.to_be_bytes()));
        }
    }
    Ok(())
}

/// Write an uncompressed NBT file with an unnamed root tag.
pub fn write(root: &Tag) -> Result<Vec<u8>> {
    let mut out = vec![root.id()];
    write_string(&mut out, "")?;
    write_payload(&mut out, root)?;
    Ok(out)
}

// ---------- modified UTF-8 ----------

/// Java's modified UTF-8: UTF-16 units encoded one by one, and NUL as two bytes.
fn to_mutf8(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for u in s.encode_utf16() {
        match u {
            0x0001..=0x007F => out.push(u as u8),
            0x0000 | 0x0080..=0x07FF => {
                out.push(0xC0 | (u >> 6) as u8);
                out.push(0x80 | (u & 0x3F) as u8);
            }
            _ => {
                out.push(0xE0 | (u >> 12) as u8);
                out.push(0x80 | ((u >> 6) & 0x3F) as u8);
                out.push(0x80 | (u & 0x3F) as u8);
            }
        }
    }
    out
}

fn from_mutf8(b: &[u8]) -> String {
    let mut units = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as u16;
        if c & 0x80 == 0 {
            units.push(c);
            i += 1;
        } else if c & 0xE0 == 0xC0 && i + 1 < b.len() {
            units.push(((c & 0x1F) << 6) | (b[i + 1] as u16 & 0x3F));
            i += 2;
        } else if c & 0xF0 == 0xE0 && i + 2 < b.len() {
            units.push(((c & 0x0F) << 12) | ((b[i + 1] as u16 & 0x3F) << 6) | (b[i + 2] as u16 & 0x3F));
            i += 3;
        } else {
            units.push(0xFFFD);
            i += 1;
        }
    }
    String::from_utf16_lossy(&units)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_tag_type() {
        let root = Tag::Compound(vec![
            ("b".into(), Tag::Byte(-3)),
            ("s".into(), Tag::Short(300)),
            ("i".into(), Tag::Int(-70000)),
            ("l".into(), Tag::Long(1 << 40)),
            ("f".into(), Tag::Float(1.5)),
            ("d".into(), Tag::Double(-2.25)),
            ("ba".into(), Tag::ByteArray(vec![1, -1])),
            ("str".into(), Tag::String("Hé 🎮 \0 end".into())),
            ("list".into(), Tag::List(10, vec![Tag::Compound(vec![("x".into(), Tag::Int(1))])])),
            ("empty".into(), Tag::List(0, vec![])),
            ("ia".into(), Tag::IntArray(vec![1, 2])),
            ("la".into(), Tag::LongArray(vec![-1])),
        ]);
        let bytes = write(&root).unwrap();
        assert_eq!(read(&bytes).unwrap(), root);
    }

    #[test]
    fn modified_utf8_matches_java() {
        // NUL is two bytes, and an emoji is a surrogate pair of three bytes each.
        assert_eq!(to_mutf8("\0"), vec![0xC0, 0x80]);
        assert_eq!(to_mutf8("🎮").len(), 6);
        assert_eq!(from_mutf8(&to_mutf8("Café 🎮")), "Café 🎮");
    }

    /// Real files the game wrote (paths in MIO_NBT_FILES, separated by `;`)
    /// must come back byte for byte. Run with `--ignored`.
    #[test]
    #[ignore]
    fn round_trips_files_the_game_wrote() {
        let files = std::env::var("MIO_NBT_FILES").expect("set MIO_NBT_FILES");
        for f in files.split(';').filter(|f| !f.is_empty()) {
            let b = std::fs::read(f).unwrap();
            let t = read(&b).unwrap();
            assert_eq!(write(&t).unwrap(), b, "{f}");
            let n = match t.get("servers") {
                Some(Tag::List(_, v)) => v.len(),
                _ => 0,
            };
            println!("ok: {} bytes, {n} servers", b.len());
        }
    }

    #[test]
    fn rejects_broken_data() {
        assert!(read(&[]).is_err());
        assert!(read(&[10, 0, 0, 1, 0]).is_err()); // ends inside a compound
        assert!(read(&[10, 0, 0, 7, 0, 1, b'a', 0x7F, 0xFF, 0xFF, 0xFF]).is_err()); // huge array
        assert!(read(&[10, 0, 0, 99, 0, 0]).is_err()); // unknown tag type
    }
}
