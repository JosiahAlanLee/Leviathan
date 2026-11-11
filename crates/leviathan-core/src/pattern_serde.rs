use core::fmt;
use core::marker::PhantomData;

use serde::de::{Error, IgnoredAny, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{AutomationRow, NoteCell, NUM_LANES, ROWS_PER_PATTERN};

struct Rows<T>([T; ROWS_PER_PATTERN]);

impl<T: Serialize> Serialize for Rows<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.as_slice().serialize(serializer)
    }
}

impl<'de, T: Copy + Default + Deserialize<'de>> Deserialize<'de> for Rows<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RowVisitor<T>(PhantomData<T>);

        impl<'de, T: Copy + Default + Deserialize<'de>> Visitor<'de> for RowVisitor<T> {
            type Value = Rows<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "exactly {ROWS_PER_PATTERN} rows")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut rows = [T::default(); ROWS_PER_PATTERN];
                for (index, row) in rows.iter_mut().enumerate() {
                    *row = seq
                        .next_element()?
                        .ok_or_else(|| A::Error::invalid_length(index, &self))?;
                }
                if seq.next_element::<IgnoredAny>()?.is_some() {
                    return Err(A::Error::invalid_length(ROWS_PER_PATTERN + 1, &self));
                }
                Ok(Rows(rows))
            }
        }

        deserializer.deserialize_tuple(ROWS_PER_PATTERN, RowVisitor(PhantomData))
    }
}

pub(super) mod lanes {
    use super::*;

    pub fn serialize<S: Serializer>(
        lanes: &[[NoteCell; ROWS_PER_PATTERN]; NUM_LANES],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        lanes.map(Rows).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<[[NoteCell; ROWS_PER_PATTERN]; NUM_LANES], D::Error> {
        let lanes = <[Rows<NoteCell>; NUM_LANES]>::deserialize(deserializer)?;
        Ok(lanes.map(|rows| rows.0))
    }
}

pub(super) mod automation {
    use super::*;

    pub fn serialize<S: Serializer>(
        rows: &[AutomationRow; ROWS_PER_PATTERN],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        Rows(*rows).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<[AutomationRow; ROWS_PER_PATTERN], D::Error> {
        Rows::deserialize(deserializer).map(|rows| rows.0)
    }
}
