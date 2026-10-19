//! `Model.generate_unique_secure_token(length: n)`: the analyzer knows
//! it as a String class method on every AR model, the runtime
//! `Base.generate_unique_secure_token(length)` answers it, and
//! `lower::secure_token::rewrite_node` moves the keyword to that
//! positional argument (Rails' default length for the bare call).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::session::analyze_and_lower;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  create_table :oauth_applications do |t|\n    t.string :client_secret\n    t.string :name\n  end\nend\n";

fn lowered(model_body: &str) -> (Vec<String>, String) {
    let files: Vec<(String, String)> = vec![
        ("db/schema.rb".into(), SCHEMA.into()),
        (
            "app/models/application_record.rb".into(),
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n".into(),
        ),
        (
            "app/models/oauth_application.rb".into(),
            format!("class OauthApplication < ApplicationRecord\n{model_body}end\n"),
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.into_bytes())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    let failures: Vec<String> = analyze_and_lower(&mut app)
        .iter()
        .filter(|d| matches!(d.code(), "send_dispatch_failed" | "unsupported"))
        .map(|d| d.message.clone())
        .collect();
    let src = roundhouse::emit::ruby::emit_lowered_models(&app)
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("models/oauth_application.rb"))
        .map(|f| f.content.clone())
        .expect("oauth_application.rb emitted");
    (failures, src)
}

#[test]
fn the_length_keyword_becomes_the_positional_argument() {
    let (failures, src) = lowered(
        "  LENGTH = 32\n\n  before_validation :set_secret\n\n  def set_secret\n    self.client_secret ||= self.class.generate_unique_secure_token(length: 48)\n  end\n\n  def self.fresh_secret = generate_unique_secure_token(length: LENGTH)\n",
    );
    assert!(failures.is_empty(), "{failures:?}");
    assert!(src.contains("self.class.generate_unique_secure_token(48)"), "{src}");
    assert!(src.contains("generate_unique_secure_token(OauthApplication::LENGTH)"), "{src}");
    assert!(!src.contains("length:"), "{src}");
}

#[test]
fn the_bare_call_takes_rails_default_length() {
    let (failures, src) = lowered(
        "  def self.fresh_secret\n    OauthApplication.generate_unique_secure_token\n  end\n",
    );
    assert!(failures.is_empty(), "{failures:?}");
    assert!(src.contains("OauthApplication.generate_unique_secure_token(24)"), "{src}");
}

#[test]
fn a_models_own_generator_keeps_its_spelling() {
    let (failures, src) = lowered(
        "  def self.generate_unique_secure_token(length: 24)\n    \"fixed\"\n  end\n\n  def self.fresh_secret\n    generate_unique_secure_token(length: 48)\n  end\n",
    );
    assert!(failures.is_empty(), "{failures:?}");
    assert!(src.contains("generate_unique_secure_token(length: 48)"), "{src}");
}

/// The emitted program mints tokens of the asked length through the
/// runtime method, keyword or bare.
#[test]
fn the_emitted_program_mints_tokens_of_the_asked_length() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy\n",
            "  has_many :comments, dependent: :destroy\n\n  def self.mint_secret(n) = generate_unique_secure_token(length: n)\n\n  def self.mint_default_secret = generate_unique_secure_token\n",
        )
        .edit(
            "test/models/article_test.rb",
            "  test \"creates an article with valid attributes\" do",
            "  test \"mints secure tokens of the asked length\" do\n    assert_equal 48, Article.mint_secret(48).length\n    assert_equal 24, Article.mint_default_secret.length\n    assert_not_equal Article.mint_secret(48), Article.mint_secret(48)\n    assert_match(/\\A[A-Za-z0-9]+\\z/, Article.mint_secret(12))\n  end\n\n  test \"creates an article with valid attributes\" do",
        )
        .run_test("test/models/article_test.rb")
        .assert_passes();
}
