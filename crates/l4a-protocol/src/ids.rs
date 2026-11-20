use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum IdParseError {
    #[error("ID 不能为空")]
    Empty,
    #[error("Workshop ID 必须为正整数: {0}")]
    InvalidWorkshopId(String),
}

/// 逻辑 Addon 唯一标识符。
///
/// 优先使用 `ws:<workshop_id>`，纯本地未知来源模组则使用 `hash:<content_sha256>`。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AddonId(pub String);

impl AddonId {
    pub fn from_workshop_id(id: WorkshopId) -> Self {
        Self(format!("ws:{}", id.0))
    }

    pub fn from_content_hash(hash: &str) -> Self {
        Self(format!("hash:{}", hash.trim().to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AddonId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Steam Workshop PublishedFileId (u64)。
///
/// 序列化为十进制字符串 (避免 JavaScript `Number.MAX_SAFE_INTEGER` 精度问题)。
/// 反序列化接受字符串 (`"123456"`) 和整数 (`123456`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkshopId(pub u64);

impl WorkshopId {
    pub fn new(raw: u64) -> Result<Self, IdParseError> {
        if raw == 0 {
            return Err(IdParseError::InvalidWorkshopId("0".to_string()));
        }
        Ok(Self(raw))
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for WorkshopId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for WorkshopId {
    type Err = IdParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(IdParseError::Empty);
        }
        let parsed = trimmed
            .parse::<u64>()
            .map_err(|_| IdParseError::InvalidWorkshopId(trimmed.to_string()))?;
        Self::new(parsed)
    }
}

impl Serialize for WorkshopId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for WorkshopId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct WorkshopIdVisitor;

        impl<'de> de::Visitor<'de> for WorkshopIdVisitor {
            type Value = WorkshopId;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a positive workshop id string or unsigned integer")
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                WorkshopId::new(value).map_err(de::Error::custom)
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                if value <= 0 {
                    return Err(de::Error::custom("workshop id must be positive"));
                }
                WorkshopId::new(value as u64).map_err(de::Error::custom)
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                WorkshopId::from_str(value).map_err(de::Error::custom)
            }
        }

        deserializer.deserialize_any(WorkshopIdVisitor)
    }
}

macro_rules! string_id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(raw: impl Into<String>) -> Result<Self, IdParseError> {
                let s = raw.into();
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    return Err(IdParseError::Empty);
                }
                Ok(Self(trimmed.to_string()))
            }

            pub fn generate() -> Self {
                Self(uuid::Uuid::new_v4().to_string())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

string_id_type!(BundleId);
string_id_type!(GroupId);
string_id_type!(ProfileId);
string_id_type!(IssueId);
string_id_type!(TaskId);
string_id_type!(ContributorId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workshop_id_serializes_as_string_and_accepts_both_string_and_number() {
        let id = WorkshopId::new(3_141_592_653).unwrap();
        let encoded = serde_json::to_string(&id).unwrap();
        assert_eq!(encoded, "\"3141592653\"");

        let from_str: WorkshopId = serde_json::from_str("\"3141592653\"").unwrap();
        let from_num: WorkshopId = serde_json::from_str("3141592653").unwrap();
        assert_eq!(from_str, id);
        assert_eq!(from_num, id);

        assert!(serde_json::from_str::<WorkshopId>("\"0\"").is_err());
        assert!(serde_json::from_str::<WorkshopId>("0").is_err());
        assert!(serde_json::from_str::<WorkshopId>("\"abc\"").is_err());
    }

    #[test]
    fn addon_id_helpers_produce_stable_prefixes() {
        let wid = WorkshopId::new(42).unwrap();
        assert_eq!(AddonId::from_workshop_id(wid).as_str(), "ws:42");
        assert_eq!(
            AddonId::from_content_hash(" AbCd ").as_str(),
            "hash:abcd"
        );
    }
}
