//! `store_accessor :col, :key, …` (ActiveRecord::Store over a JSON
//! column) — `lower::store_accessor` synthesizes one untyped
//! reader/writer pair per key over the serialized column, through the
//! column's own `JsonColumn` seam, whether the declaration sits in the
//! model body or in a concern's `included do`.

use roundhouse::analyze::{diagnose, Analyzer};
use roundhouse::dialect::MethodReceiver;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_model_to_library_class;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  create_table :users do |t|\n    t.string :name\n    t.jsonb :preferences\n    t.text :settings\n  end\nend\n";

fn app_with(model_body: &str, extra: Vec<(&str, &str)>) -> roundhouse::App {
    let mut files: Vec<(String, String)> = vec![
        ("db/schema.rb".into(), SCHEMA.into()),
        (
            "app/models/user.rb".into(),
            format!("class User < ApplicationRecord\n{model_body}end\n"),
        ),
    ];
    files.extend(extra.into_iter().map(|(p, c)| (p.to_string(), c.to_string())));
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.into_bytes()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

fn user_class(app: &roundhouse::App) -> roundhouse::dialect::LibraryClass {
    let user = app.models.iter().find(|m| m.name.0.as_str() == "User").expect("User model");
    lower_model_to_library_class(user, &app.schema)
}

fn instance_method<'a>(
    lc: &'a roundhouse::dialect::LibraryClass,
    name: &str,
) -> Option<&'a roundhouse::dialect::MethodDef> {
    lc.methods
        .iter()
        .find(|m| m.name.as_str() == name && m.receiver == MethodReceiver::Instance)
}

fn user_source(app: &roundhouse::App) -> String {
    roundhouse::emit::ruby::emit_lowered_models(app)
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("models/user.rb"))
        .map(|f| f.content.clone())
        .expect("user.rb emitted")
}

#[test]
fn keys_synthesize_an_untyped_reader_and_writer_over_the_json_column() {
    let app = app_with("  store_accessor :preferences, :theme, :locale\n", vec![]);
    let lc = user_class(&app);

    let writer = instance_method(&lc, "theme=").expect("writer synthesized");
    assert!(writer.mutates_self, "writer assigns the column back");
    assert!(instance_method(&lc, "locale").is_some());
    assert!(instance_method(&lc, "locale=").is_some());
    // Rails' `store_accessor` defines no `?` predicate.
    assert!(instance_method(&lc, "theme?").is_none());

    let src = user_source(&app);
    assert!(
        src.contains(r#"JsonColumn.read_key(@preferences, "theme")"#),
        "reader renders over the serialized column:\n{src}"
    );
    assert!(
        src.contains(r#"@preferences = JsonColumn.write_key(@preferences, "theme", value)"#),
        "writer re-serializes the whole object back into the column:\n{src}"
    );
    // The column accessor itself is untouched: `@preferences` stays
    // the serialized text every synthesized path already moves.
    assert!(
        src.contains("def preferences\n"),
        "the column reader stays:\n{src}"
    );
}

#[test]
fn a_declaration_inside_a_concerns_included_block_expands_on_the_includer() {
    // Jumpstart's `User::Theme`: the macro runs on the includer's class
    // body, so `User#theme` has to exist exactly as for a direct call.
    let app = app_with(
        "  include Theme\n",
        vec![(
            "app/models/user/theme.rb",
            "module User::Theme\n  extend ActiveSupport::Concern\n\n  included do\n    store_accessor :preferences, :theme\n  end\n\n  def dark_theme? = theme == \"dark\"\nend\n",
        )],
    );
    let lc = user_class(&app);
    assert!(instance_method(&lc, "theme=").is_some(), "writer carried to the includer");
    let src = user_source(&app);
    assert!(
        src.contains(r#"JsonColumn.read_key(@preferences, "theme")"#),
        "the includer's model carries the accessor:\n{src}"
    );
}

#[test]
fn prefix_and_suffix_options_name_the_accessor_but_not_the_key() {
    let app = app_with(
        "  store_accessor :preferences, :theme, prefix: true\n  store_accessor :preferences, :locale, suffix: :pref\n",
        vec![],
    );
    let lc = user_class(&app);
    assert!(instance_method(&lc, "preferences_theme").is_some());
    assert!(instance_method(&lc, "preferences_theme=").is_some());
    assert!(instance_method(&lc, "theme").is_none(), "prefixed: the bare name is not defined");
    assert!(instance_method(&lc, "locale_pref").is_some());
    let src = user_source(&app);
    assert!(
        src.contains(r#"JsonColumn.read_key(@preferences, "theme")"#),
        "the JSON key stays the declared symbol:\n{src}"
    );
}

#[test]
fn a_user_defined_method_wins_over_synthesis() {
    let app = app_with(
        "  store_accessor :preferences, :theme\n\n  def theme\n    \"dark\"\n  end\n",
        vec![],
    );
    let lc = user_class(&app);
    let reader = instance_method(&lc, "theme").expect("reader present");
    let body = format!("{:?}", reader.body);
    assert!(!body.contains("JsonColumn"), "the model's own method must win: {body}");
}

#[test]
fn a_non_json_column_or_a_splatted_key_list_leaves_the_declaration_unclaimed() {
    // `settings` is a text column with no JSON coder — Rails would read
    // it through `store`'s YAML coder, which the JSON seam cannot.
    let app = app_with("  store_accessor :settings, :theme\n", vec![]);
    let lc = user_class(&app);
    assert!(instance_method(&lc, "theme").is_none(), "nothing synthesized over a YAML column");

    // The names come from a constant's value, not the declaration.
    let app = app_with(
        "  ROLES = [:admin, :member]\n  store_accessor :preferences, *ROLES\n",
        vec![],
    );
    let lc = user_class(&app);
    assert!(instance_method(&lc, "admin").is_none(), "a splatted list is not expanded");
}

#[test]
fn the_analyzer_dispatches_the_accessors_as_gradual() {
    // The declaration names keys, not types; a call site that reads a
    // key resolves (no `send_dispatch_failed`) and stays gradual.
    let mut app = app_with(
        "  store_accessor :preferences, :theme\n\n  def dark_theme? = theme == \"dark\"\n",
        vec![(
            "app/views/users/show.html.erb",
            "<p><%= @user.theme %> <%= @user.dark_theme? %></p>\n",
        )],
    );
    Analyzer::new(&app).analyze(&mut app);
    let failures: Vec<String> = diagnose(&app)
        .iter()
        .filter(|d| d.code() == "send_dispatch_failed")
        .map(|d| format!("{d:?}"))
        .collect();
    assert!(failures.is_empty(), "every accessor resolves: {failures:?}");
}
