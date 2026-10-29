use std::fmt;

use serde::de::{self, DeserializeSeed, IntoDeserializer, Visitor};
use serde::ser::{self, Serialize};

#[derive(Debug, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl ser::Error for Error {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Error(msg.to_string())
    }
}

impl de::Error for Error {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Error(msg.to_string())
    }
}

type Result<T> = std::result::Result<T, Error>;

pub fn to_bytes<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    value.serialize(&mut Ser(&mut out))?;
    Ok(out)
}

pub fn from_bytes<T: de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    let mut de = De(bytes);
    let value = T::deserialize(&mut de)?;
    if !de.0.is_empty() {
        return Err(Error(format!("{} trailing bytes", de.0.len())));
    }
    Ok(value)
}

struct Ser<'a>(&'a mut Vec<u8>);

impl Ser<'_> {
    fn len(&mut self, n: usize) {
        self.0.extend_from_slice(&(n as u64).to_le_bytes());
    }
    fn index(&mut self, i: u32) {
        self.0.extend_from_slice(&i.to_le_bytes());
    }
}

macro_rules! ser_num {
    ($($f:ident $t:ty),*) => {$(
        fn $f(self, v: $t) -> Result<()> {
            self.0.extend_from_slice(&v.to_le_bytes());
            Ok(())
        }
    )*};
}

impl<'a, 'b> ser::Serializer for &'a mut Ser<'b> {
    type Ok = ();
    type Error = Error;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    fn is_human_readable(&self) -> bool {
        false
    }

    ser_num!(serialize_i8 i8, serialize_i16 i16, serialize_i32 i32, serialize_i64 i64,
        serialize_u8 u8, serialize_u16 u16, serialize_u32 u32, serialize_u64 u64,
        serialize_f32 f32, serialize_f64 f64);

    fn serialize_bool(self, v: bool) -> Result<()> {
        self.0.push(u8::from(v));
        Ok(())
    }
    fn serialize_char(self, v: char) -> Result<()> {
        self.serialize_u32(u32::from(v))
    }
    fn serialize_str(self, v: &str) -> Result<()> {
        self.serialize_bytes(v.as_bytes())
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<()> {
        self.len(v.len());
        self.0.extend_from_slice(v);
        Ok(())
    }
    fn serialize_none(self) -> Result<()> {
        self.0.push(0);
        Ok(())
    }
    fn serialize_some<T: ?Sized + Serialize>(self, v: &T) -> Result<()> {
        self.0.push(1);
        v.serialize(self)
    }
    fn serialize_unit(self) -> Result<()> {
        Ok(())
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<()> {
        Ok(())
    }
    fn serialize_unit_variant(self, _: &'static str, i: u32, _: &'static str) -> Result<()> {
        self.index(i);
        Ok(())
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(self, _: &'static str, v: &T) -> Result<()> {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _: &'static str,
        i: u32,
        _: &'static str,
        v: &T,
    ) -> Result<()> {
        self.index(i);
        v.serialize(self)
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Self> {
        let len = len.ok_or_else(|| Error("sequence of unknown length".into()))?;
        self.len(len);
        Ok(self)
    }
    fn serialize_tuple(self, _: usize) -> Result<Self> {
        Ok(self)
    }
    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self> {
        Ok(self)
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        i: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self> {
        self.index(i);
        Ok(self)
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Self> {
        let len = len.ok_or_else(|| Error("map of unknown length".into()))?;
        self.len(len);
        Ok(self)
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self> {
        Ok(self)
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        i: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self> {
        self.index(i);
        Ok(self)
    }
}

macro_rules! ser_compound {
    ($trait:ident, $method:ident) => {
        impl ser::$trait for &mut Ser<'_> {
            type Ok = ();
            type Error = Error;
            fn $method<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<()> {
                v.serialize(&mut **self)
            }
            fn end(self) -> Result<()> {
                Ok(())
            }
        }
    };
}

ser_compound!(SerializeSeq, serialize_element);
ser_compound!(SerializeTuple, serialize_element);
ser_compound!(SerializeTupleStruct, serialize_field);
ser_compound!(SerializeTupleVariant, serialize_field);

impl ser::SerializeMap for &mut Ser<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, k: &T) -> Result<()> {
        k.serialize(&mut **self)
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<()> {
        v.serialize(&mut **self)
    }
    fn end(self) -> Result<()> {
        Ok(())
    }
}

impl ser::SerializeStruct for &mut Ser<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, _: &'static str, v: &T) -> Result<()> {
        v.serialize(&mut **self)
    }
    fn end(self) -> Result<()> {
        Ok(())
    }
}

impl ser::SerializeStructVariant for &mut Ser<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, _: &'static str, v: &T) -> Result<()> {
        v.serialize(&mut **self)
    }
    fn end(self) -> Result<()> {
        Ok(())
    }
}

struct De<'de>(&'de [u8]);

impl<'de> De<'de> {
    fn take(&mut self, n: usize) -> Result<&'de [u8]> {
        if self.0.len() < n {
            return Err(Error("unexpected end of input".into()));
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into().unwrap())
    }
    fn len(&mut self) -> Result<usize> {
        usize::try_from(u64::from_le_bytes(self.array()?)).map_err(|_| Error("length".into()))
    }
    fn index(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }
}

macro_rules! de_num {
    ($($f:ident $visit:ident $t:ty),*) => {$(
        fn $f<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
            v.$visit(<$t>::from_le_bytes(self.array()?))
        }
    )*};
}

impl<'de> de::Deserializer<'de> for &mut De<'de> {
    type Error = Error;

    fn is_human_readable(&self) -> bool {
        false
    }

    fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value> {
        Err(Error(
            "deserialize_any on a format that is not self-describing".into(),
        ))
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value> {
        Err(Error(
            "deserialize_ignored_any on a format that is not self-describing".into(),
        ))
    }

    de_num!(deserialize_i8 visit_i8 i8, deserialize_i16 visit_i16 i16,
        deserialize_i32 visit_i32 i32, deserialize_i64 visit_i64 i64,
        deserialize_u8 visit_u8 u8, deserialize_u16 visit_u16 u16,
        deserialize_u32 visit_u32 u32, deserialize_u64 visit_u64 u64,
        deserialize_f32 visit_f32 f32, deserialize_f64 visit_f64 f64);

    fn deserialize_bool<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        match self.take(1)?[0] {
            0 => v.visit_bool(false),
            1 => v.visit_bool(true),
            b => Err(Error(format!("bool byte {b}"))),
        }
    }
    fn deserialize_char<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        let n = u32::from_le_bytes(self.array()?);
        v.visit_char(char::from_u32(n).ok_or_else(|| Error("char".into()))?)
    }
    fn deserialize_str<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        let n = self.len()?;
        let bytes = self.take(n)?;
        v.visit_borrowed_str(std::str::from_utf8(bytes).map_err(|_| Error("utf-8".into()))?)
    }
    fn deserialize_string<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_str(v)
    }
    fn deserialize_bytes<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        let n = self.len()?;
        v.visit_borrowed_bytes(self.take(n)?)
    }
    fn deserialize_byte_buf<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.deserialize_bytes(v)
    }
    fn deserialize_option<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        match self.take(1)?[0] {
            0 => v.visit_none(),
            1 => v.visit_some(self),
            b => Err(Error(format!("option byte {b}"))),
        }
    }
    fn deserialize_unit<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        v.visit_unit()
    }
    fn deserialize_unit_struct<V: Visitor<'de>>(self, _: &'static str, v: V) -> Result<V::Value> {
        v.visit_unit()
    }
    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        v: V,
    ) -> Result<V::Value> {
        v.visit_newtype_struct(self)
    }
    fn deserialize_seq<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        let n = self.len()?;
        v.visit_seq(Counted(self, n))
    }
    fn deserialize_tuple<V: Visitor<'de>>(self, n: usize, v: V) -> Result<V::Value> {
        v.visit_seq(Counted(self, n))
    }
    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        n: usize,
        v: V,
    ) -> Result<V::Value> {
        v.visit_seq(Counted(self, n))
    }
    fn deserialize_map<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        let n = self.len()?;
        v.visit_map(Counted(self, n))
    }
    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        fields: &'static [&'static str],
        v: V,
    ) -> Result<V::Value> {
        v.visit_seq(Counted(self, fields.len()))
    }
    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _: &'static str,
        _: &'static [&'static str],
        v: V,
    ) -> Result<V::Value> {
        v.visit_enum(self)
    }
    fn deserialize_identifier<V: Visitor<'de>>(self, _: V) -> Result<V::Value> {
        Err(Error("identifiers are variant indices here".into()))
    }
}

struct Counted<'a, 'de>(&'a mut De<'de>, usize);

impl<'de> de::SeqAccess<'de> for Counted<'_, 'de> {
    type Error = Error;
    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>> {
        if self.1 == 0 {
            return Ok(None);
        }
        self.1 -= 1;
        seed.deserialize(&mut *self.0).map(Some)
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.1)
    }
}

impl<'de> de::MapAccess<'de> for Counted<'_, 'de> {
    type Error = Error;
    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>> {
        if self.1 == 0 {
            return Ok(None);
        }
        self.1 -= 1;
        seed.deserialize(&mut *self.0).map(Some)
    }
    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value> {
        seed.deserialize(&mut *self.0)
    }
}

impl<'de> de::EnumAccess<'de> for &mut De<'de> {
    type Error = Error;
    type Variant = Self;
    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self)> {
        let index = self.index()?;
        let value = seed.deserialize(index.into_deserializer())?;
        Ok((value, self))
    }
}

impl<'de> de::VariantAccess<'de> for &mut De<'de> {
    type Error = Error;
    fn unit_variant(self) -> Result<()> {
        Ok(())
    }
    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value> {
        seed.deserialize(self)
    }
    fn tuple_variant<V: Visitor<'de>>(self, n: usize, v: V) -> Result<V::Value> {
        v.visit_seq(Counted(self, n))
    }
    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        v: V,
    ) -> Result<V::Value> {
        v.visit_seq(Counted(self, fields.len()))
    }
}
