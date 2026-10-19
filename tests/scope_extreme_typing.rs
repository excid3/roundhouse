//! `published.maximum(:published_at)` — an extreme over a NAMED SCOPE.
//! The schema-indexed typing (`relation_extreme_ty`) declined every
//! scope receiver, since a scope may group and then the extreme is a
//! per-group Hash. A scope whose body is a builder chain that never
//! groups is proven scalar at registration (`ClassInfo::scalar_scopes`),
//! and the extreme over it types as the column's value or nil. A
//! grouping scope, a scope chained through one, and one the chain
//! reader cannot read, still decline.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, Analyzer};
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  create_table :announcements do |t|\n    t.string :kind\n    t.boolean :published\n    t.datetime :published_at\n  end\nend\n";

fn failures(model_body: &str) -> Vec<String> {
    let files: Vec<(String, String)> = vec![
        ("db/schema.rb".into(), SCHEMA.into()),
        (
            "app/models/application_record.rb".into(),
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n".into(),
        ),
        (
            "app/models/announcement.rb".into(),
            format!("class Announcement < ApplicationRecord\n{model_body}end\n"),
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.into_bytes())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    Analyzer::new(&app).analyze(&mut app);
    diagnose(&app)
        .iter()
        .filter(|d| matches!(d.code(), "send_dispatch_failed" | "unsupported" | "incompatible_binop"))
        .map(|d| d.message.clone())
        .collect()
}

const SCOPES: &str = "  scope :recent, -> { published.order(published_at: :desc) }\n  scope :published, -> { where(published: true) }\n  scope :since, ->(t) { published.where(\"published_at > ?\", t) }\n  scope :by_kind, -> { group(:kind) }\n  scope :by_kind_recent, -> { by_kind.order(:kind) }\n";

#[test]
fn an_extreme_over_a_scalar_scope_types_as_the_column_or_nil() {
    // Jumpstart's `Announcement.published.maximum(:published_at)`: the
    // newest publication date, compared to the user's last-seen time.
    // `recent` is declared before the `published` it chains through,
    // `since` takes an argument, and the last chain passes a builder
    // on the way to the scope: all one row set.
    let found = failures(&format!(
        "{SCOPES}\n  def self.latest = published.maximum(:published_at)\n\n  def self.newest = Announcement.recent.maximum(:published_at)\n\n  def self.newest_since(t) = since(t).maximum(:published_at)\n\n  def self.newest_of(kind) = where(kind: kind).published.maximum(:published_at)\n\n  def self.fresh?(seen_at)\n    latest = self.published.maximum(:published_at)\n    !latest.nil? && latest > seen_at\n  end\n"
    ));
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn an_extreme_over_a_grouping_scope_still_declines() {
    // `by_kind.maximum(:published_at)` is a Hash per kind, which the
    // schema-indexed typing does not fabricate from a scope name.
    let found = failures(&format!(
        "{SCOPES}\n  def self.latest_by_kind = by_kind.maximum(:published_at)\n\n  def self.latest_by_kind_recent = by_kind_recent.maximum(:published_at)\n"
    ));
    assert_eq!(found.iter().filter(|m| m.contains("maximum")).count(), 2, "{found:?}");
}

#[test]
fn a_scope_that_reads_an_unreadable_body_is_not_proven_scalar() {
    // A block makes the chain unreadable; the scope stays unproven and
    // the extreme over it declines, as before.
    let found = failures(
        "  scope :odd, -> { where(published: true).select { |a| a.id.odd? } }\n\n  def self.latest_odd = odd.maximum(:published_at)\n",
    );
    assert!(found.iter().any(|m| m.contains("maximum")), "{found:?}");
}
