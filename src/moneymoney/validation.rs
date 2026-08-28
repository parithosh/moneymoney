//! Shared validation for values that cross CLI and MCP write paths.

use std::str::FromStr as _;

use iban::{BaseIban, IbanLike as _};
use rust_decimal::Decimal;
use time::Date;

use super::MoneyMoneyError;

pub fn parse_amount(value: &str) -> Result<Decimal, MoneyMoneyError> {
    let trimmed = value.trim();
    let amount = Decimal::from_str(trimmed)
        .map_err(|error| MoneyMoneyError::InvalidAmount(format!("'{value}': {error}")))?;
    if amount.is_zero() {
        return Err(MoneyMoneyError::InvalidAmount(
            "amount must be non-zero".to_owned(),
        ));
    }
    if amount.scale() > 4 {
        return Err(MoneyMoneyError::InvalidAmount(format!(
            "'{value}' has too many decimal places (max 4)"
        )));
    }
    Ok(amount)
}

pub fn require_positive_amount(amount: Decimal) -> Result<(), MoneyMoneyError> {
    if amount <= Decimal::ZERO {
        return Err(MoneyMoneyError::InvalidAmount(
            "transfer amounts must be positive".to_owned(),
        ));
    }
    if amount.scale() > 4 {
        return Err(MoneyMoneyError::InvalidAmount(
            "transfer amounts may have at most 4 decimal places".to_owned(),
        ));
    }
    Ok(())
}

pub fn normalize_iban(value: &str) -> Result<String, MoneyMoneyError> {
    BaseIban::from_str(value.trim())
        .map(|iban| iban.electronic_str().to_owned())
        .map_err(|error| MoneyMoneyError::InvalidIban(format!("'{value}': {error}")))
}

pub fn validate_date_range(from: Date, to: Date) -> Result<(), MoneyMoneyError> {
    if from <= to {
        Ok(())
    } else {
        Err(MoneyMoneyError::InvalidDateRange { from, to })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "tests assert on static fixtures")]
mod tests {
    use super::*;

    #[test]
    fn amount_rules_are_shared() {
        assert_eq!(parse_amount("12.3400").unwrap().scale(), 4);
        assert!(parse_amount("0").is_err());
        assert!(parse_amount("1.00001").is_err());
        assert!(require_positive_amount(Decimal::ONE).is_ok());
        assert!(require_positive_amount(Decimal::new(100_001, 5)).is_err());
        assert!(require_positive_amount(-Decimal::ONE).is_err());
    }

    #[test]
    fn iban_is_validated_and_normalized() {
        assert_eq!(
            normalize_iban("DE89 3704 0044 0532 0130 00").unwrap(),
            "DE89370400440532013000"
        );
        assert!(normalize_iban("DE001234").is_err());
    }
}
