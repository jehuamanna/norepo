//! Strongly-typed UUID newtypes per aggregate so a `UserId` cannot be passed
//! where an `OrgId` is expected.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

use crate::error::StoreError;

macro_rules! id_newtype {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            pub fn from_str_strict(s: &str) -> Result<Self, StoreError> {
                Uuid::parse_str(s)
                    .map(Self)
                    .map_err(|_| StoreError::InvalidUuid(s.to_owned()))
            }

            pub fn as_str(&self) -> String {
                self.0.to_string()
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $name {
            type Err = StoreError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::from_str_strict(s)
            }
        }
    };
}

id_newtype!(UserId);
id_newtype!(OrgId);
id_newtype!(DepartmentId);
id_newtype!(TeamId);
id_newtype!(ProjectId);
id_newtype!(NoteId);
id_newtype!(MembershipId);
id_newtype!(TeamMemberId);
id_newtype!(TeamProjectId);
id_newtype!(InviteId);
id_newtype!(SessionId);
id_newtype!(AttachmentId);

pub const SYSTEM_ORG_ID: &str = "00000000-0000-0000-0000-000000000000";
pub const LOCAL_ORG_ID: &str = "00000000-0000-0000-0000-000000000001";
