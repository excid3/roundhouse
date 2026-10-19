//! `self.class::NAME` reads a constant off the class `self` belongs to
//! at runtime, not off the lexical scope the `def` was written in: a
//! concern's method reads the INCLUDER's constant, a subclass's method
//! reads one its parent declared. The typer resolves the read on the
//! class `self` has at the site and qualifies the path, so every
//! emitter prints an ordinary constant; a concern several classes
//! include keeps the runtime lookup, spelled as one.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::diagnose;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::session::analyze_and_lower;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  create_table :account_users do |t|\n    t.jsonb :roles\n    t.string :name\n  end\nend\n";

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

const ACCOUNT_USER: (&str, &str) = (
    "app/models/account_user.rb",
    "class AccountUser < ApplicationRecord\n  ROLES = [:admin, :billing].freeze\n  include Roles\nend\n",
);

/// Jumpstart's `AccountUser::Roles#active_roles`: the compact module
/// name gives the `def` no lexical path to `AccountUser::ROLES`.
const ROLES_CONCERN: (&str, &str) = (
    "app/models/account_user/roles.rb",
    "module AccountUser::Roles\n  extend ActiveSupport::Concern\n\n  def active_roles\n    self.class::ROLES.map(&:to_s)\n  end\nend\n",
);

/// Ingested, typed and lowered, with the seeded files replaced by a
/// test's own copy of the same path.
fn app_with(files: &[(&str, &str)]) -> roundhouse::App {
    let mut all: Vec<(String, String)> = vec![
        ("db/schema.rb".into(), SCHEMA.into()),
        ("app/models/application_record.rb".into(), APPLICATION_RECORD.into()),
    ];
    all.extend(files.iter().map(|(p, c)| (p.to_string(), c.to_string())));
    let tree: HashMap<PathBuf, Vec<u8>> =
        all.into_iter().map(|(p, c)| (PathBuf::from(p), c.into_bytes())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    analyze_and_lower(&mut app);
    app
}

fn messages(app: &roundhouse::App, codes: &[&str]) -> Vec<String> {
    diagnose(app)
        .iter()
        .filter(|d| codes.contains(&d.code()))
        .map(|d| d.message.clone())
        .collect()
}

fn failures(app: &roundhouse::App) -> Vec<String> {
    messages(app, &["send_dispatch_failed", "unsupported"])
}

/// The concern's methods, emitted where the concern is: Ruby keeps the
/// module (a library class) and `include`s it.
fn roles_source(app: &roundhouse::App) -> String {
    roundhouse::emit::ruby::emit_library(app)
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("account_user/roles.rb"))
        .map(|f| f.content.clone())
        .expect("roles.rb emitted")
}

#[test]
fn a_concern_method_reads_the_includers_constant() {
    let app = app_with(&[ACCOUNT_USER, ROLES_CONCERN]);
    let found = failures(&app);
    assert!(found.is_empty(), "{found:?}");
    let src = roles_source(&app);
    assert!(src.contains("AccountUser::ROLES.map"), "the includer's constant:\n{src}");
}

#[test]
fn the_lexical_scope_is_not_consulted() {
    // The concern declares a `ROLES` of its own; `self.class::ROLES`
    // still reads the includer's.
    let app = app_with(&[
        ACCOUNT_USER,
        (
            ROLES_CONCERN.0,
            "module AccountUser::Roles\n  extend ActiveSupport::Concern\n\n  ROLES = [\"concern\"].freeze\n\n  def active_roles\n    self.class::ROLES.map(&:to_s)\n  end\nend\n",
        ),
    ]);
    let found = failures(&app);
    assert!(found.is_empty(), "{found:?}");
    let src = roles_source(&app);
    assert!(
        src.contains("AccountUser::ROLES.map") && !src.contains("Roles::ROLES"),
        "the includer's constant, not the concern's:\n{src}"
    );
}

#[test]
fn a_concern_with_several_includers_keeps_the_runtime_lookup() {
    // Jumpstart's `AccountUser::Roles` is included by `AccountUser` and
    // `AccountInvitation`, each with its own `ROLES`: the module's copy
    // runs under both, so it stays a lookup on the runtime class, on
    // the residue ledger, typed as what every includer answers.
    let app = app_with(&[
        ACCOUNT_USER,
        (
            "app/models/account_invitation.rb",
            "class AccountInvitation < ApplicationRecord\n  ROLES = [:member].freeze\n  include AccountUser::Roles\nend\n",
        ),
        ROLES_CONCERN,
    ]);
    let found = failures(&app);
    assert!(found.is_empty(), "{found:?}");
    let residue = messages(&app, &["lower_residue"]);
    assert!(residue.iter().any(|m| m.contains("self.class::ROLES")), "{residue:?}");
    let src = roles_source(&app);
    assert!(src.contains("self.class.const_get(:ROLES).map"), "the runtime lookup, spelled:\n{src}");
}

#[test]
fn several_includers_answering_from_one_class_qualify_the_read() {
    // Both includers inherit `ROLES` from `ApplicationRecord`: one
    // declaring class, so the module's copy names it outright.
    let app = app_with(&[
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\n  ROLES = [:admin].freeze\nend\n",
        ),
        ("app/models/account_user.rb", "class AccountUser < ApplicationRecord\n  include Roles\nend\n"),
        (
            "app/models/account_invitation.rb",
            "class AccountInvitation < ApplicationRecord\n  include AccountUser::Roles\nend\n",
        ),
        ROLES_CONCERN,
    ]);
    let found = failures(&app);
    assert!(found.is_empty(), "{found:?}");
    let src = roles_source(&app);
    assert!(src.contains("ApplicationRecord::ROLES.map"), "one declaring class:\n{src}");
}

#[test]
fn an_includer_without_the_constant_makes_the_shared_read_unsupported() {
    let app = app_with(&[
        ACCOUNT_USER,
        (
            "app/models/account_invitation.rb",
            "class AccountInvitation < ApplicationRecord\n  include AccountUser::Roles\nend\n",
        ),
        ROLES_CONCERN,
    ]);
    let found = failures(&app);
    assert!(
        found.iter().any(|m| m.contains("constant not supported (all targets): self.class::ROLES")),
        "{found:?}"
    );
}

#[test]
fn an_inherited_constant_resolves_through_the_parent() {
    let app = app_with(&[
        ("app/clients/application_client.rb", "class ApplicationClient\n  LIMIT = 3\nend\n"),
        (
            "app/clients/example_client.rb",
            "class ExampleClient < ApplicationClient\n  def limit = self.class::LIMIT + 1\nend\n",
        ),
    ]);
    let found = failures(&app);
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_constant_only_subclasses_declare_stays_unsupported() {
    // Jumpstart's `ApplicationClient#base_uri = self.class::BASE_URI`:
    // each client declares its own, so no one class answers the read,
    // and the report says what was written.
    let app = app_with(&[
        (
            "app/clients/application_client.rb",
            "class ApplicationClient\n  def base_uri = self.class::BASE_URI\nend\n",
        ),
        (
            "app/clients/example_client.rb",
            "class ExampleClient < ApplicationClient\n  BASE_URI = \"https://example.com\"\nend\n",
        ),
    ]);
    let found = failures(&app);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("constant not supported (all targets): self.class::BASE_URI"), "{found:?}");
}
