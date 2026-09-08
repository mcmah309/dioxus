use crate::{FileData, FormValue, SerializedFileData};
use serde::de::{
    DeserializeOwned, IntoDeserializer, Visitor,
    value::{MapAccessDeserializer, MapDeserializer, SeqDeserializer},
};
use serde::{Deserializer, forward_to_deserialize_any};
use serde_json::{Error, Value};
use std::collections::BTreeMap;

pub(super) fn from_values<T: DeserializeOwned>(
    values: Vec<(String, FormValue)>,
) -> Result<T, Error> {
    let mut files: Vec<(FileData, SerializedFileData)> = Vec::new();
    let mut fields = BTreeMap::<String, Vec<FormField>>::new();
    for (key, value) in values {
        let field = match value {
            FormValue::Text(text) => FormField::Value(Value::String(text)),
            FormValue::File(Some(file)) => {
                let metadata = SerializedFileData::from_file_data(&file);
                let value = serde_json::to_value(&metadata)?;
                let index = files.len();
                files.push((file, metadata));
                FormField::File {
                    index,
                    metadata: value,
                }
            }
            FormValue::File(None) => {
                FormField::Value(serde_json::to_value(SerializedFileData::empty())?)
            }
        };
        fields.entry(key).or_default().push(field);
    }
    let fields = fields
        .into_iter()
        .map(|(key, mut values)| {
            let value = if values.len() == 1 {
                values.pop().unwrap()
            } else {
                FormField::Multiple(values)
            };
            (key, value)
        })
        .collect();
    crate::file_data::with_form_files(files, || T::deserialize(FormField::Map(fields)))
}

// Keep file handles separate from their metadata. Ordinary metadata/JSON fields see the same
// representation as before; FileData fields can clone the original handle during deserialization.
enum FormField {
    Value(Value),
    File { index: usize, metadata: Value },
    Multiple(Vec<FormField>),
    Map(BTreeMap<String, FormField>),
}

impl<'de> IntoDeserializer<'de, Error> for FormField {
    type Deserializer = Self;

    fn into_deserializer(self) -> Self {
        self
    }
}

impl<'de> Deserializer<'de> for FormField {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self {
            Self::Value(value)
            | Self::File {
                metadata: value, ..
            } => value.deserialize_any(visitor),
            Self::Multiple(values) => {
                SeqDeserializer::new(values.into_iter()).deserialize_any(visitor)
            }
            Self::Map(fields) => MapDeserializer::new(fields.into_iter()).deserialize_any(visitor),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        if name == crate::file_data::FILE_DATA_NEWTYPE {
            if let Self::File { index, .. } = self {
                return visitor.visit_u64(index as u64);
            }
        }
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_some(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        match self {
            Self::Value(value)
            | Self::File {
                metadata: value, ..
            } => value.deserialize_enum(name, variants, visitor),
            Self::Map(fields) if fields.len() == 1 => {
                MapAccessDeserializer::new(MapDeserializer::new(fields.into_iter()))
                    .deserialize_enum(name, variants, visitor)
            }
            Self::Map(_) => Err(serde::de::Error::invalid_value(
                serde::de::Unexpected::Map,
                &"a map with a single key",
            )),
            Self::Multiple(_) => Err(serde::de::Error::invalid_type(
                serde::de::Unexpected::Seq,
                &visitor,
            )),
        }
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf
        unit unit_struct seq tuple tuple_struct map struct identifier ignored_any
    }
}
