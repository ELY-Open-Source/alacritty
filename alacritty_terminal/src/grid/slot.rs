//! A row as storage keeps it: whole, or packed while idle.

use std::fmt;
use std::sync::OnceLock;

#[cfg(feature = "serde")]
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::Row;

/// A row packed small, which unpacks to the same row.
pub trait PackedRow<T>: Send + Sync {
    fn unpack(&self) -> Row<T>;
}

/// Packs a row into its small form.
pub type Packer<T> = fn(&Row<T>) -> Box<dyn PackedRow<T>>;

/// A stored row; a packed one unpacks on demand while read.
pub(super) enum Slot<T> {
    Row(Row<T>),
    Packed(Box<dyn PackedRow<T>>, OnceLock<Row<T>>),
}

impl<T> Slot<T> {
    #[inline]
    pub fn row(&self) -> &Row<T> {
        match self {
            Slot::Row(row) => row,
            Slot::Packed(packed, row) => row.get_or_init(|| packed.unpack()),
        }
    }

    /// The row to change, unpacked for good.
    #[inline]
    pub fn row_mut(&mut self) -> &mut Row<T> {
        if let Slot::Packed(packed, row) = self {
            let row = row.take().unwrap_or_else(|| packed.unpack());
            *self = Slot::Row(row);
        }
        match self {
            Slot::Row(row) => row,
            Slot::Packed(..) => unreachable!("unpacked above"),
        }
    }

    pub fn into_row(self) -> Row<T> {
        match self {
            Slot::Row(row) => row,
            Slot::Packed(packed, mut row) => row.take().unwrap_or_else(|| packed.unpack()),
        }
    }

    /// Packs a whole row, or drops a packed one's copy; whether it did.
    pub fn pack(&mut self, packer: Packer<T>) -> bool {
        match self {
            Slot::Row(row) => {
                *self = Slot::Packed(packer(row), OnceLock::new());
                true
            },
            Slot::Packed(_, row) => row.take().is_some(),
        }
    }
}

impl<T> From<Row<T>> for Slot<T> {
    fn from(row: Row<T>) -> Self {
        Slot::Row(row)
    }
}

impl<T: Clone> Clone for Slot<T> {
    fn clone(&self) -> Self {
        Slot::Row(self.row().clone())
    }
}

impl<T: fmt::Debug> fmt::Debug for Slot<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.row().fmt(f)
    }
}

impl<T: PartialEq> PartialEq for Slot<T> {
    fn eq(&self, other: &Self) -> bool {
        self.row() == other.row()
    }
}

impl<T: PartialEq> PartialEq<Row<T>> for Slot<T> {
    fn eq(&self, other: &Row<T>) -> bool {
        self.row() == other
    }
}

#[cfg(feature = "serde")]
impl<T: Serialize> Serialize for Slot<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.row().serialize(serializer)
    }
}

#[cfg(feature = "serde")]
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Slot<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Row::deserialize(deserializer).map(Slot::Row)
    }
}
