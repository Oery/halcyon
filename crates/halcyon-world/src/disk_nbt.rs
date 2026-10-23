use crate::LoadError;

/// Owned NBT DOM for the cold load path only.
///
/// Anvil chunk data uses tags the network codec never sees
/// (`ByteArray`, `IntArray`, `LongArray`), and values must outlive the
/// read buffer, so this is deliberately separate from
/// `mc_protocol::types::Nbt` (borrowed, encode-only). Only bounded supported
/// sign text retains sanitized values after import; its wire snapshot is encoded
/// on change, never by movement or once per observer.
#[derive(Debug, Clone)]
pub enum DiskNbt {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<u8>),
    Str(String),
    List(u8, Vec<DiskNbt>),
    Compound(Vec<(String, DiskNbt)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl DiskNbt {
    /// Read one root `TAG_Compound` (type byte + name prefix + payload).
    pub fn read_root(buf: &mut &[u8]) -> Result<DiskNbt, LoadError> {
        let ty = pop(buf).ok_or_else(|| LoadError::BadNbt {
            chunk: String::new(),
            reason: "empty buffer".to_owned(),
        })?;
        if ty != 10 {
            return Err(LoadError::BadNbt {
                chunk: String::new(),
                reason: format!("root tag must be compound, got {ty}"),
            });
        }
        let _name = read_str(buf).map_err(|reason| LoadError::BadNbt {
            chunk: String::new(),
            reason: format!("bad root name: {reason}"),
        })?;
        read_payload(buf, 10).map_err(|reason| LoadError::BadNbt { chunk: String::new(), reason })
    }

    /// Compound child lookup; `None` for missing keys and non-compounds.
    pub fn get(&self, key: &str) -> Option<&DiskNbt> {
        match self {
            DiskNbt::Compound(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> Option<i32> {
        match self {
            DiskNbt::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_i8(&self) -> Option<i8> {
        match self {
            DiskNbt::Byte(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            DiskNbt::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[DiskNbt]> {
        match self {
            DiskNbt::List(_, items) => Some(items),
            _ => None,
        }
    }

    pub fn as_compound(&self) -> Option<&[(String, DiskNbt)]> {
        match self {
            DiskNbt::Compound(fields) => Some(fields),
            _ => None,
        }
    }

    pub fn as_int_array(&self) -> Option<&[i32]> {
        match self {
            DiskNbt::IntArray(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_long_array(&self) -> Option<&[i64]> {
        match self {
            DiskNbt::LongArray(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f32> {
        match self {
            DiskNbt::Float(v) => Some(*v),
            _ => None,
        }
    }
}

fn pop(buf: &mut &[u8]) -> Option<u8> {
    let (first, rest) = buf.split_first()?;
    *buf = rest;
    Some(*first)
}

fn take_n<'b>(buf: &mut &'b [u8], n: usize) -> Result<&'b [u8], String> {
    if buf.len() < n {
        return Err(format!("need {n} bytes, have {}", buf.len()));
    }
    let (head, rest) = buf.split_at(n);
    *buf = rest;
    Ok(head)
}

fn read_u16(buf: &mut &[u8]) -> Result<u16, String> {
    Ok(u16::from_be_bytes(take_n(buf, 2)?.try_into().unwrap()))
}

fn read_str(buf: &mut &[u8]) -> Result<String, String> {
    let len = read_u16(buf)? as usize;
    let bytes = take_n(buf, len)?;
    String::from_utf8(bytes.to_vec()).map_err(|e| format!("bad utf-8: {e}"))
}

fn read_payload(buf: &mut &[u8], ty: u8) -> Result<DiskNbt, String> {
    match ty {
        1 => Ok(DiskNbt::Byte(take_n(buf, 1)?[0] as i8)),
        2 => Ok(DiskNbt::Short(i16::from_be_bytes(take_n(buf, 2)?.try_into().unwrap()))),
        3 => Ok(DiskNbt::Int(i32::from_be_bytes(take_n(buf, 4)?.try_into().unwrap()))),
        4 => Ok(DiskNbt::Long(i64::from_be_bytes(take_n(buf, 8)?.try_into().unwrap()))),
        5 => Ok(DiskNbt::Float(f32::from_be_bytes(take_n(buf, 4)?.try_into().unwrap()))),
        6 => Ok(DiskNbt::Double(f64::from_be_bytes(take_n(buf, 8)?.try_into().unwrap()))),
        7 => {
            let len = i32::from_be_bytes(take_n(buf, 4)?.try_into().unwrap());
            if len < 0 {
                return Err(format!("negative bytearray len {len}"));
            }
            Ok(DiskNbt::ByteArray(take_n(buf, len as usize)?.to_vec()))
        }
        8 => Ok(DiskNbt::Str(read_str(buf)?)),
        9 => {
            let elem = take_n(buf, 1)?[0];
            let len = i32::from_be_bytes(take_n(buf, 4)?.try_into().unwrap());
            if len < 0 {
                return Err(format!("negative list len {len}"));
            }
            let mut items = Vec::with_capacity(len.min(4096) as usize);
            for _ in 0..len {
                items.push(read_payload(buf, elem)?);
            }
            Ok(DiskNbt::List(elem, items))
        }
        10 => {
            let mut fields = Vec::new();
            loop {
                let tag = pop(buf).ok_or("compound missing TAG_End")?;
                if tag == 0 {
                    break;
                }
                let name = read_str(buf)?;
                fields.push((name, read_payload(buf, tag)?));
            }
            Ok(DiskNbt::Compound(fields))
        }
        11 => {
            let len = i32::from_be_bytes(take_n(buf, 4)?.try_into().unwrap());
            if len < 0 {
                return Err(format!("negative intarray len {len}"));
            }
            let raw = take_n(buf, len as usize * 4)?;
            Ok(DiskNbt::IntArray(
                raw.as_chunks::<4>().0.iter().map(|c| i32::from_be_bytes(*c)).collect(),
            ))
        }
        12 => {
            let len = i32::from_be_bytes(take_n(buf, 4)?.try_into().unwrap());
            if len < 0 {
                return Err(format!("negative longarray len {len}"));
            }
            let raw = take_n(buf, len as usize * 8)?;
            Ok(DiskNbt::LongArray(
                raw.as_chunks::<8>().0.iter().map(|c| i64::from_be_bytes(*c)).collect(),
            ))
        }
        other => Err(format!("unknown tag type {other}")),
    }
}

#[cfg(test)]
#[path = "tests/tests_disk_nbt.rs"]
mod tests;
