#![forbid(unsafe_code)]
//! Read-only Ledger inspector.
//!
//! The store is a SQLite database using STRICT tables, so a host whose system
//! `sqlite3` predates 3.37 cannot open it. This binary links the same bundled
//! SQLite the writer used, which makes it the only reliable way to read a
//! Ledger on such a host — and the only way to invoke chain verification at
//! all.

use std::path::PathBuf;
use std::process::ExitCode;

use aw_contracts::ids::{AttemptId, LedgerEventId};
use aw_contracts::ledger::LedgerEventKind;
use aw_ledger::{verify_chain, LedgerStore, StoredRecord};
use clap::{Parser, Subcommand, ValueEnum};

const EXIT_FAILURE: u8 = 12;

#[derive(Debug, Parser)]
#[command(name = "aw-ledger", version, about = "Inspect and verify an AW Ledger")]
struct Cli {
    /// Directory holding `ledger.db`.
    #[arg(long, value_name = "DIR")]
    ledger: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Recompute every digest and parent link, then report the record count.
    ///
    /// Exits non-zero on the first broken invariant, naming the sequence.
    Verify,
    /// List records as one line each: sequence, kind, schema, record digest.
    List {
        /// Show only records of this kind.
        #[arg(long, value_enum)]
        kind: Option<EventKindArg>,
        /// Show only records scoped to this attempt.
        #[arg(long, value_name = "ID")]
        attempt: Option<String>,
    },
    /// Print the canonical body bytes of one record.
    ///
    /// This is the surface for auditing content-freedom: whatever this prints
    /// is exactly what the Ledger stored.
    Body {
        /// Record identity, as printed by `list`.
        #[arg(value_name = "EVENT_ID")]
        event_id: String,
    },
}

/// Event kinds a reader can filter on, spelled as they appear in storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum EventKindArg {
    PostToolUsePlan,
    PreToolUseGate,
    ContextAdoption,
    ProviderInvoked,
    EvidenceStored,
    ReceiptStored,
}

impl From<EventKindArg> for LedgerEventKind {
    fn from(value: EventKindArg) -> Self {
        match value {
            EventKindArg::PostToolUsePlan => Self::PostToolUsePlan,
            EventKindArg::PreToolUseGate => Self::PreToolUseGate,
            EventKindArg::ContextAdoption => Self::ContextAdoption,
            EventKindArg::ProviderInvoked => Self::ProviderInvoked,
            EventKindArg::EvidenceStored => Self::EvidenceStored,
            EventKindArg::ReceiptStored => Self::ReceiptStored,
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("aw-ledger: {error}");
            ExitCode::from(EXIT_FAILURE)
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let store = LedgerStore::open(&cli.ledger)?;
    match cli.command {
        Command::Verify => {
            let count = verify_chain(&store)?;
            println!("verified {count} record(s); chain intact");
        }
        Command::List { kind, attempt } => {
            let records = match (kind, attempt) {
                (Some(_), Some(_)) => return Err("--kind and --attempt cannot be combined".into()),
                (Some(kind), None) => store.events_by_kind(kind.into())?,
                (None, Some(attempt)) => store.events_for_attempt(&AttemptId::parse(attempt)?)?,
                (None, None) => all_records(&store)?,
            };
            for record in &records {
                print_record(record);
            }
            println!("{} record(s)", records.len());
        }
        Command::Body { event_id } => {
            let id = LedgerEventId::parse(event_id)?;
            let bytes = store.record_body_bytes(&id)?;
            println!("{}", String::from_utf8(bytes)?);
        }
    }
    Ok(())
}

/// Lists every record by unioning the kind-filtered queries.
///
/// The store deliberately exposes no unbounded scan, so an "everything" view is
/// assembled from the bounded ones and re-sorted.
fn all_records(store: &LedgerStore) -> Result<Vec<StoredRecord>, Box<dyn std::error::Error>> {
    let mut records = Vec::new();
    for kind in [
        LedgerEventKind::PostToolUsePlan,
        LedgerEventKind::PreToolUseGate,
        LedgerEventKind::ContextAdoption,
        LedgerEventKind::ProviderInvoked,
        LedgerEventKind::EvidenceStored,
        LedgerEventKind::ReceiptStored,
    ] {
        records.extend(store.events_by_kind(kind)?);
    }
    records.sort_by_key(|record| record.header.sequence);
    Ok(records)
}

fn print_record(record: &StoredRecord) {
    let scope = record
        .scope
        .as_ref()
        .and_then(|scope| scope.tool_use_id.as_ref())
        .map_or("-", |id| id.as_str());
    println!(
        "{:>6}  {}  {}  {}  {}  tool_use={}",
        record.header.sequence,
        record.header.id.as_str(),
        kind_label(record.header.kind),
        record.header.schema,
        record.record_digest.as_str(),
        scope,
    );
}

fn kind_label(kind: LedgerEventKind) -> &'static str {
    match kind {
        LedgerEventKind::PostToolUsePlan => "post_tool_use_plan",
        LedgerEventKind::PreToolUseGate => "pre_tool_use_gate",
        LedgerEventKind::ContextAdoption => "context_adoption",
        LedgerEventKind::ProviderInvoked => "provider_invoked",
        LedgerEventKind::EvidenceStored => "evidence_stored",
        LedgerEventKind::ReceiptStored => "receipt_stored",
    }
}
