//! Structures and traits related to sorting.

#[cfg(not(feature = "std"))]
use alloc::{
    format,
    string::{String, ToString as _},
    vec,
    vec::Vec,
};

use iroha_data_model_derive::model;
use iroha_schema::IntoSchema;
use parity_scale_codec::{Decode, Encode};
use serde::{Deserialize, Serialize};

pub use self::model::*;
use crate::{name::Name, prelude::*};

#[model]
mod model {
    use super::*;

    /// Struct for sorting requests
    #[derive(
        Debug, Clone, Default, PartialEq, Eq, Decode, Encode, Deserialize, Serialize, IntoSchema,
    )]
    pub struct Sorting {
        /// Sort query result using [`Name`] of the key in [`Asset`]'s metadata.
        pub sort_by_metadata_key: Option<Name>,
    }
}

impl Sorting {
    /// Creates a sorting by [`Name`] of the key.
    pub fn by_metadata_key(key: Name) -> Self {
        Self {
            sort_by_metadata_key: Some(key),
        }
    }
}

pub mod prelude {
    //! Prelude: re-export most commonly used traits, structs and macros from this module.
    pub use super::*;
}
