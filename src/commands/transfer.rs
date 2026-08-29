//! `mm transfer {create,direct-debit,batch}` — initiate SEPA payments via
//! MoneyMoney's AppleScript interface.
//!
//! All three verbs are safe-by-construction: MoneyMoney either opens a
//! pre-filled transfer window (default) or drops the payment into the
//! Ausgangskorb (`--into-outbox`). Either way the user still has to
//! confirm and enter a TAN before money moves.

use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use rust_decimal::Decimal;
use time::Date;

use crate::applescript::{OsascriptRunner, string_expression};
use crate::commands::accounts::{annotate_with_bank, fetch_all};
use crate::moneymoney::MoneyMoneyError;
use crate::moneymoney::resolver::Resolver;
use crate::moneymoney::validation::{normalize_iban, require_positive_amount};

/// Parameters for `mm transfer create`.
pub struct CreateTransferOptions {
    pub from: String,
    pub to_iban: String,
    pub to_name: Option<String>,
    pub amount: Decimal,
    pub purpose: Option<String>,
    pub endtoend_reference: Option<String>,
    pub scheduled_date: Option<Date>,
    pub into_outbox: bool,
    pub aliases: HashMap<String, String>,
    pub format: Option<crate::output::OutputFormat>,
}

/// Parameters for `mm transfer direct-debit`.
pub struct CreateDirectDebitOptions {
    pub from: String,
    pub debtor_iban: String,
    pub debtor_name: Option<String>,
    pub amount: Decimal,
    pub purpose: Option<String>,
    pub mandate_reference: String,
    pub mandate_date: Option<Date>,
    pub scheduled_date: Option<Date>,
    pub into_outbox: bool,
    pub aliases: HashMap<String, String>,
    pub format: Option<crate::output::OutputFormat>,
}

/// Parameters for `mm transfer batch`.
pub struct BatchTransferOptions {
    pub sepa_xml_path: PathBuf,
    pub direct_debit: bool,
    pub format: Option<crate::output::OutputFormat>,
}

/// `mm transfer create` — build script, dispatch, echo a confirmation.
pub async fn run_create<R: OsascriptRunner>(
    runner: &R,
    opts: &CreateTransferOptions,
) -> anyhow::Result<()> {
    let raw = fetch_all(runner).await?;
    let rows = annotate_with_bank(raw);
    let resolver = Resolver::new(rows, opts.aliases.clone());
    let from_row = resolver.resolve(&opts.from)?;

    let script = build_create_transfer_script(&from_row.account.account_number, opts)?;
    runner.run(&script).await?;
    announce(opts.into_outbox, opts.format, "bank transfer");
    Ok(())
}

/// `mm transfer direct-debit` — build script, dispatch, echo a confirmation.
pub async fn run_direct_debit<R: OsascriptRunner>(
    runner: &R,
    opts: &CreateDirectDebitOptions,
) -> anyhow::Result<()> {
    let raw = fetch_all(runner).await?;
    let rows = annotate_with_bank(raw);
    let resolver = Resolver::new(rows, opts.aliases.clone());
    let from_row = resolver.resolve(&opts.from)?;

    let script = build_direct_debit_script(&from_row.account.account_number, opts)?;
    runner.run(&script).await?;
    announce(opts.into_outbox, opts.format, "direct debit");
    Ok(())
}

/// Prepare and retain a private snapshot of the SEPA XML while MoneyMoney
/// imports it.
pub(crate) struct PreparedBatch {
    script: String,
    #[allow(
        dead_code,
        reason = "ownership retains and deletes the staging file after AppleScript completes"
    )]
    staged: tempfile::NamedTempFile,
}

impl PreparedBatch {
    #[must_use]
    pub(crate) fn script(&self) -> &str {
        &self.script
    }
}

/// Copy a confined SEPA XML file into a private staging file.
pub(crate) fn prepare_batch(opts: &BatchTransferOptions) -> Result<PreparedBatch, MoneyMoneyError> {
    prepare_batch_in(opts, &crate::statements::container_data_root())
}

/// `mm transfer batch` — load a SEPA XML file.
pub async fn run_batch<R: OsascriptRunner>(
    runner: &R,
    opts: &BatchTransferOptions,
) -> anyhow::Result<()> {
    let prepared = prepare_batch(opts)?;
    runner.run(prepared.script()).await?;
    let verb = if opts.direct_debit {
        "batch direct debit"
    } else {
        "batch transfer"
    };
    announce(false, opts.format, verb);
    Ok(())
}

fn announce(into_outbox: bool, format: Option<crate::output::OutputFormat>, verb: &str) {
    let format = crate::output::OutputFormat::resolve(format);
    let destination = if into_outbox {
        "queued into the MoneyMoney outbox"
    } else {
        "opened in a MoneyMoney payment window; confirm in the GUI and enter TAN"
    };
    match format {
        crate::output::OutputFormat::Json | crate::output::OutputFormat::Ndjson => {
            let payload = serde_json::json!({
                "action": verb,
                "delivery": if into_outbox { "outbox" } else { "window" },
                "message": destination,
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&payload).unwrap_or_default()
            );
        }
        crate::output::OutputFormat::Table => {
            println!("{verb}: {destination}");
        }
    }
}

/// Build the `create bank transfer` `AppleScript` string.
pub fn build_create_transfer_script(
    from_iban: &str,
    opts: &CreateTransferOptions,
) -> Result<String, MoneyMoneyError> {
    require_positive_amount(opts.amount)?;
    let to_iban = normalize_iban(&opts.to_iban)?;

    let mut parts = vec![
        format!("from account {}", string_expression(from_iban)),
        format!("iban {}", string_expression(&to_iban)),
        format!("amount {}", opts.amount),
    ];
    if let Some(name) = &opts.to_name {
        parts.push(format!("to {}", string_expression(name)));
    }
    if let Some(purpose) = &opts.purpose {
        parts.push(format!("purpose {}", string_expression(purpose)));
    }
    if let Some(e2e) = &opts.endtoend_reference {
        parts.push(format!("endtoend reference {}", string_expression(e2e)));
    }
    if let Some(date) = opts.scheduled_date {
        parts.push(format!(
            "scheduled date {}",
            string_expression(&date.to_string())
        ));
    }
    if opts.into_outbox {
        parts.push(r#"into "outbox""#.to_owned());
    }

    Ok(format!(
        "tell application \"MoneyMoney\" to create bank transfer {}",
        parts.join(" ")
    ))
}

/// Build the `create direct debit` `AppleScript` string.
pub fn build_direct_debit_script(
    from_iban: &str,
    opts: &CreateDirectDebitOptions,
) -> Result<String, MoneyMoneyError> {
    require_positive_amount(opts.amount)?;
    let debtor_iban = normalize_iban(&opts.debtor_iban)?;

    let mut parts = vec![
        format!("from account {}", string_expression(from_iban)),
        format!("iban {}", string_expression(&debtor_iban)),
        format!("amount {}", opts.amount),
        format!(
            "mandate reference {}",
            string_expression(&opts.mandate_reference)
        ),
    ];
    if let Some(name) = &opts.debtor_name {
        parts.push(format!("for {}", string_expression(name)));
    }
    if let Some(purpose) = &opts.purpose {
        parts.push(format!("purpose {}", string_expression(purpose)));
    }
    if let Some(date) = opts.mandate_date {
        parts.push(format!(
            "mandate date {}",
            string_expression(&date.to_string())
        ));
    }
    if let Some(date) = opts.scheduled_date {
        parts.push(format!(
            "scheduled date {}",
            string_expression(&date.to_string())
        ));
    }
    if opts.into_outbox {
        parts.push(r#"into "outbox""#.to_owned());
    }

    Ok(format!(
        "tell application \"MoneyMoney\" to create direct debit {}",
        parts.join(" ")
    ))
}

fn prepare_batch_in(
    opts: &BatchTransferOptions,
    root: &Path,
) -> Result<PreparedBatch, MoneyMoneyError> {
    let (mut source, root) = open_batch_file_in(&opts.sepa_xml_path, root)?;
    let mut staged = tempfile::Builder::new()
        .prefix(".mm-batch-")
        .suffix(".xml")
        .tempfile_in(&root)
        .map_err(|error| {
            MoneyMoneyError::InvalidBatchFile(format!(
                "failed to create a private staging file in '{}': {error}",
                root.display()
            ))
        })?;
    io::copy(&mut source, staged.as_file_mut()).map_err(|error| {
        MoneyMoneyError::InvalidBatchFile(format!(
            "failed to snapshot '{}': {error}",
            opts.sepa_xml_path.display()
        ))
    })?;
    let script = build_batch_script_for_path(staged.path(), opts.direct_debit)?;
    Ok(PreparedBatch { script, staged })
}

fn open_batch_file_in(path: &Path, root: &Path) -> Result<(File, PathBuf), MoneyMoneyError> {
    if root.as_os_str().is_empty() {
        return Err(MoneyMoneyError::InvalidBatchFile(
            "MoneyMoney container root could not be determined".to_owned(),
        ));
    }
    let root = std::fs::canonicalize(root).map_err(|error| {
        MoneyMoneyError::InvalidBatchFile(format!(
            "MoneyMoney container '{}' is unavailable: {error}",
            root.display()
        ))
    })?;
    let path = std::fs::canonicalize(path).map_err(|error| {
        MoneyMoneyError::InvalidBatchFile(format!("'{}': {error}", path.display()))
    })?;
    if !path.starts_with(&root) {
        return Err(MoneyMoneyError::InvalidBatchFile(format!(
            "'{}' is outside '{}'",
            path.display(),
            root.display()
        )));
    }
    let is_xml = path
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("xml"));
    if !is_xml {
        return Err(MoneyMoneyError::InvalidBatchFile(format!(
            "'{}' must have an .xml extension",
            path.display()
        )));
    }
    let relative = path.strip_prefix(&root).map_err(|error| {
        MoneyMoneyError::InvalidBatchFile(format!(
            "'{}' is not reachable from '{}': {error}",
            path.display(),
            root.display()
        ))
    })?;
    let source = open_confined_regular_file(&root, relative).map_err(|error| {
        MoneyMoneyError::InvalidBatchFile(format!(
            "failed to securely open '{}': {error}",
            path.display()
        ))
    })?;
    if !source
        .metadata()
        .map_err(|error| {
            MoneyMoneyError::InvalidBatchFile(format!(
                "failed to inspect '{}': {error}",
                path.display()
            ))
        })?
        .is_file()
    {
        return Err(MoneyMoneyError::InvalidBatchFile(format!(
            "'{}' is not a regular file",
            path.display()
        )));
    }
    Ok((source, root))
}

#[cfg(unix)]
fn open_confined_regular_file(root: &Path, relative: &Path) -> io::Result<File> {
    use std::path::Component;

    use rustix::fs::{Mode, OFlags, open, openat};

    let directory_flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::DIRECTORY | OFlags::NOFOLLOW;
    let file_flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
    let mut directory = open(root, directory_flags, Mode::empty())?;
    let mut components = relative.components().peekable();

    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "batch path contains a non-normal component",
            ));
        };
        let is_file = components.peek().is_none();
        let opened = openat(
            &directory,
            name,
            if is_file { file_flags } else { directory_flags },
            Mode::empty(),
        )?;
        if is_file {
            return Ok(File::from(opened));
        }
        directory = opened;
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "batch path names the container root",
    ))
}

#[cfg(not(unix))]
fn open_confined_regular_file(_root: &Path, _relative: &Path) -> io::Result<File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "secure batch staging requires Unix file-descriptor APIs",
    ))
}

fn build_batch_script_for_path(path: &Path, direct_debit: bool) -> Result<String, MoneyMoneyError> {
    let path = path.to_str().ok_or_else(|| {
        MoneyMoneyError::InvalidBatchFile(format!("'{}' is not valid UTF-8", path.display()))
    })?;
    let verb = if direct_debit {
        "create batch direct debit"
    } else {
        "create batch transfer"
    };
    Ok(format!(
        "tell application \"MoneyMoney\" to {verb} from POSIX file ({})",
        string_expression(path)
    ))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "tests assert on hand-constructed fixtures"
)]
mod tests {
    use std::str::FromStr;

    use super::*;

    fn base_create() -> CreateTransferOptions {
        CreateTransferOptions {
            from: "ING/Girokonto".to_owned(),
            to_iban: "DE89370400440532013000".to_owned(),
            to_name: Some("Alice".to_owned()),
            amount: Decimal::from_str("12.34").unwrap(),
            purpose: Some("Rent".to_owned()),
            endtoend_reference: None,
            scheduled_date: None,
            into_outbox: false,
            aliases: HashMap::new(),
            format: None,
        }
    }

    #[test]
    fn create_transfer_script_matches_expected_shape() {
        let script =
            build_create_transfer_script("DE92500105175437633269", &base_create()).unwrap();
        assert_eq!(
            script,
            r#"tell application "MoneyMoney" to create bank transfer from account "DE92500105175437633269" iban "DE89370400440532013000" amount 12.34 to "Alice" purpose "Rent""#
        );
    }

    #[test]
    fn into_outbox_appends_clause() {
        let mut opts = base_create();
        opts.into_outbox = true;
        let script = build_create_transfer_script("DE92500105175437633269", &opts).unwrap();
        assert!(script.ends_with(r#"into "outbox""#));
    }

    #[test]
    fn quote_in_purpose_is_encoded() {
        let mut opts = base_create();
        opts.purpose = Some(r#"He said "hi""#.to_owned());
        let script = build_create_transfer_script("DE92500105175437633269", &opts).unwrap();
        assert!(script.contains("(ASCII character 34)"));
        assert!(!script.contains(r#"purpose "He said "hi""#));
    }

    #[test]
    fn transfer_requires_valid_iban_and_positive_amount() {
        let mut opts = base_create();
        opts.to_iban = "DE001234".to_owned();
        assert!(matches!(
            build_create_transfer_script("DE92500105175437633269", &opts),
            Err(MoneyMoneyError::InvalidIban(_))
        ));

        opts.to_iban = "DE89370400440532013000".to_owned();
        opts.amount = Decimal::NEGATIVE_ONE;
        assert!(matches!(
            build_create_transfer_script("DE92500105175437633269", &opts),
            Err(MoneyMoneyError::InvalidAmount(_))
        ));
    }

    #[test]
    fn direct_debit_emits_mandate() {
        let opts = CreateDirectDebitOptions {
            from: "ING/Girokonto".to_owned(),
            debtor_iban: "DE89370400440532013000".to_owned(),
            debtor_name: Some("Tenant".to_owned()),
            amount: Decimal::from_str("500.00").unwrap(),
            purpose: None,
            mandate_reference: "MANDATE-42".to_owned(),
            mandate_date: None,
            scheduled_date: None,
            into_outbox: true,
            aliases: HashMap::new(),
            format: None,
        };
        let script = build_direct_debit_script("DE92500105175437633269", &opts).unwrap();
        assert!(script.contains(r#"mandate reference "MANDATE-42""#));
        assert!(script.contains(r#"for "Tenant""#));
        assert!(script.ends_with(r#"into "outbox""#));
    }

    #[test]
    fn batch_script_groups_posix_file_expression() {
        let script = build_batch_script_for_path(Path::new("/tmp/sepa.xml"), false).unwrap();
        assert_eq!(
            script,
            r#"tell application "MoneyMoney" to create batch transfer from POSIX file ("/tmp/sepa.xml")"#
        );

        let quoted =
            build_batch_script_for_path(Path::new(r#"/tmp/sepa "quoted".xml"#), false).unwrap();
        assert_eq!(
            quoted,
            r#"tell application "MoneyMoney" to create batch transfer from POSIX file ("/tmp/sepa " & (ASCII character 34) & "quoted" & (ASCII character 34) & ".xml")"#
        );
    }

    #[test]
    fn batch_file_is_snapshotted_inside_allowed_root() {
        let base = tempfile::tempdir().unwrap();
        let allowed = base.path().join("allowed");
        let inside = allowed.join("batch.xml");
        let outside = base.path().join("outside.xml");
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::write(&inside, "<Document>original</Document>").unwrap();
        std::fs::write(&outside, "<Document>outside</Document>").unwrap();
        let opts = BatchTransferOptions {
            sepa_xml_path: inside.clone(),
            direct_debit: false,
            format: None,
        };

        let prepared = prepare_batch_in(&opts, &allowed).unwrap();
        let staged_path = prepared.staged.path().to_owned();
        assert!(staged_path.starts_with(std::fs::canonicalize(&allowed).unwrap()));
        assert_eq!(
            std::fs::read_to_string(&staged_path).unwrap(),
            "<Document>original</Document>"
        );
        std::fs::write(&inside, "<Document>replaced</Document>").unwrap();
        assert_eq!(
            std::fs::read_to_string(&staged_path).unwrap(),
            "<Document>original</Document>"
        );
        drop(prepared);
        assert!(!staged_path.exists());

        let outside_opts = BatchTransferOptions {
            sepa_xml_path: outside,
            direct_debit: false,
            format: None,
        };
        assert!(prepare_batch_in(&outside_opts, &allowed).is_err());
    }
}
