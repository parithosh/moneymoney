//! Process-wide write capability policy.
//!
//! Financial mutations are disabled unless the process starts with
//! `MM_ENABLE_WRITES=true`. The value is read once at startup so an MCP tool
//! cannot elevate an already-running read-only server.

use std::ffi::OsStr;

use crate::moneymoney::MoneyMoneyError;

pub const ENABLE_WRITES_ENV: &str = "MM_ENABLE_WRITES";

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct WritePolicy {
    enabled: bool,
}

impl WritePolicy {
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_value(std::env::var_os(ENABLE_WRITES_ENV).as_deref())
    }

    #[must_use]
    pub fn from_value(value: Option<&OsStr>) -> Self {
        Self {
            enabled: value == Some(OsStr::new("true")),
        }
    }

    #[must_use]
    pub const fn is_enabled(self) -> bool {
        self.enabled
    }

    pub fn require(self) -> Result<(), MoneyMoneyError> {
        if self.enabled {
            Ok(())
        } else {
            Err(MoneyMoneyError::WritesDisabled)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_are_disabled_without_exact_opt_in() {
        for value in [None, Some(""), Some("1"), Some("TRUE"), Some("yes")] {
            let value = value.map(OsStr::new);
            assert!(!WritePolicy::from_value(value).is_enabled());
        }
    }

    #[test]
    fn lowercase_true_enables_writes() {
        assert!(WritePolicy::from_value(Some(OsStr::new("true"))).is_enabled());
    }
}
