//! `mm transactions` — list transactions for an account over a date range.

use std::collections::HashMap;

use time::{Date, OffsetDateTime, UtcOffset};

use crate::applescript::{OsascriptRunner, run_plist, string_expression};
use crate::commands::accounts::{annotate_with_bank, fetch_all};
use crate::moneymoney::MoneyMoneyError;
use crate::moneymoney::resolver::Resolver;
use crate::moneymoney::types::{Transaction, TransactionsEnvelope};
use crate::moneymoney::validation::validate_date_range;
use crate::output::{DetailView, FieldFilter, FieldNames, OutputFormat, Tabular, format_list};

/// Options controlling `mm transactions`.
pub struct ListOptions {
    pub reference: String,
    pub from: Option<Date>,
    pub to: Option<Date>,
    pub search: Option<String>,
    pub limit: Option<usize>,
    pub format: Option<OutputFormat>,
    pub fields: Option<String>,
    pub aliases: HashMap<String, String>,
}

impl Tabular for Transaction {
    fn headers() -> &'static [&'static str] {
        &["Date", "Name", "Amount", "Currency", "Purpose"]
    }

    fn row(&self) -> Vec<String> {
        vec![
            self.booking_date
                .map(|d| {
                    let sys: std::time::SystemTime = d.into();
                    let odt: OffsetDateTime = sys.into();
                    odt.date().to_string()
                })
                .unwrap_or_default(),
            self.name.clone(),
            self.amount.to_string(),
            self.currency.clone(),
            truncate(&self.purpose, 80),
        ]
    }
}

impl DetailView for Transaction {
    fn fields(&self) -> Vec<(&'static str, String)> {
        vec![
            ("ID", self.id.to_string()),
            ("Account", self.account_uuid.clone()),
            ("Name", self.name.clone()),
            ("Amount", self.amount.to_string()),
            ("Currency", self.currency.clone()),
            (
                "BookingDate",
                self.booking_date.map_or_else(String::new, format_date),
            ),
            (
                "ValueDate",
                self.value_date.map_or_else(String::new, format_date),
            ),
            ("BookingText", self.booking_text.clone()),
            ("Purpose", self.purpose.clone()),
            ("CategoryUUID", self.category_uuid.clone()),
            ("Comment", self.comment.clone()),
            ("Checkmark", self.checkmark.to_string()),
            ("Booked", self.booked.to_string()),
            ("CounterpartyAccount", self.account_number.clone()),
            ("CounterpartyBankCode", self.bank_code.clone()),
        ]
    }
}

impl FieldNames for Transaction {
    fn valid_fields() -> &'static [&'static str] {
        &[
            "id",
            "account",
            "name",
            "amount",
            "currency",
            "bookingdate",
            "valuedate",
            "bookingtext",
            "purpose",
            "categoryuuid",
            "comment",
            "checkmark",
            "booked",
            "counterpartyaccount",
            "counterpartybankcode",
        ]
    }
}

fn format_date(d: plist::Date) -> String {
    let sys: std::time::SystemTime = d.into();
    let odt: OffsetDateTime = sys.into();
    odt.date().to_string()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// `mm transactions` entrypoint.
pub async fn run_list<R: OsascriptRunner>(runner: &R, opts: ListOptions) -> anyhow::Result<()> {
    // Reject invalid ranges before contacting MoneyMoney.
    let today = OffsetDateTime::now_utc().to_offset(UtcOffset::UTC).date();
    let from = opts
        .from
        .unwrap_or_else(|| today - time::Duration::days(90));
    let to = opts.to.unwrap_or(today);
    validate_date_range(from, to)?;

    let raw = fetch_all(runner).await?;
    let rows = annotate_with_bank(raw);
    let resolver = Resolver::new(rows, opts.aliases);
    let account_row = resolver.resolve(&opts.reference)?;

    let script = build_export_script(&account_row.account.account_number, from, to)?;
    let envelope: TransactionsEnvelope = run_plist(runner, &script).await?;

    // Apply search filter (case-insensitive; matches name + purpose +
    // comment — categoryUuid rarely makes a useful haystack).
    let needle = opts.search.as_deref().map(str::to_lowercase);
    let filtered: Vec<Transaction> = envelope
        .transactions
        .into_iter()
        .filter(|t| match &needle {
            Some(q) => {
                let hay = [&t.name, &t.purpose, &t.comment];
                hay.iter().any(|h| h.to_lowercase().contains(q.as_str()))
            }
            None => true,
        })
        .collect();
    let total = filtered.len();
    let truncated: Vec<Transaction> = match opts.limit {
        Some(n) => filtered.into_iter().take(n).collect(),
        None => filtered,
    };

    let format = OutputFormat::resolve(opts.format);
    let field_filter = opts
        .fields
        .as_deref()
        .map(FieldFilter::parse::<Transaction>)
        .transpose()?;

    format_list(&truncated, total, format, field_filter.as_ref())
}

/// Build the AppleScript string for `export transactions` over a date range.
/// Exposed so the MCP tool can reuse it without going through the CLI options
/// struct.
pub fn build_export_script(account: &str, from: Date, to: Date) -> Result<String, MoneyMoneyError> {
    validate_date_range(from, to)?;
    Ok(format!(
        "tell application \"MoneyMoney\" to export transactions from account {} from date {} to date {} as \"plist\"",
        string_expression(account),
        string_expression(&from.to_string()),
        string_expression(&to.to_string())
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "tests assert on static fixtures")]
mod tests {
    use super::*;

    #[test]
    fn export_script_encodes_account_and_validates_range() {
        let from = Date::from_calendar_date(2026, time::Month::January, 1).unwrap();
        let to = Date::from_calendar_date(2026, time::Month::January, 31).unwrap();
        let script = build_export_script(r#"account" & bad"#, from, to).unwrap();
        assert!(script.contains("(ASCII character 34)"));
        assert!(!script.contains(r#""account" & bad""#));
        assert!(build_export_script("account", to, from).is_err());
    }
}
