use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::fmt;
use ulid::Ulid;

macro_rules! ulid_newtype {
    ($name:ident) => {
        #[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
        pub struct $name(pub Ulid);

        impl $name {
            pub fn new() -> Self {
                Self(Ulid::new())
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
    };
}

ulid_newtype!(CommunityId);
ulid_newtype!(ChannelId);
ulid_newtype!(MessageId);

/// A 32-byte user identifier derived as SHA-256 of the user's Ed25519 verifying key.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct UserId(pub [u8; 32]);

impl UserId {
    /// Derive a `UserId` from an Ed25519 verifying key.
    pub fn from_verifying_key(key: &VerifyingKey) -> Self {
        let hash = Sha256::digest(key.as_bytes());
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&hash);
        Self(bytes)
    }
}

impl fmt::Display for UserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

// Serialize as hex string so `UserId` is usable as a JSON/msgpack map key.
impl Serialize for UserId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for UserId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct HexVisitor;
        impl<'de> serde::de::Visitor<'de> for HexVisitor {
            type Value = UserId;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "a 64-character lowercase hex string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<UserId, E> {
                if v.len() != 64 {
                    return Err(E::custom(format!(
                        "expected 64-char hex string, got {} chars",
                        v.len()
                    )));
                }
                let mut bytes = [0u8; 32];
                for (i, chunk) in v.as_bytes().chunks(2).enumerate() {
                    let hex_str = std::str::from_utf8(chunk).map_err(E::custom)?;
                    bytes[i] = u8::from_str_radix(hex_str, 16).map_err(E::custom)?;
                }
                Ok(UserId(bytes))
            }
        }
        d.deserialize_str(HexVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn community_id_display_roundtrip() {
        let id = CommunityId::new();
        let s = id.to_string();
        assert_eq!(s.len(), 26); // ULID is 26 chars
    }

    #[test]
    fn user_id_serde_roundtrip() {
        use rand::rngs::OsRng;
        let key = ed25519_dalek::SigningKey::generate(&mut OsRng);
        let uid = UserId::from_verifying_key(&key.verifying_key());
        let json = serde_json::to_string(&uid).unwrap();
        let restored: UserId = serde_json::from_str(&json).unwrap();
        assert_eq!(uid, restored);
    }
}
