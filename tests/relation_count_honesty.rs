//! Rails count contracts for DISTINCT, GROUP BY, and HAVING (#343).
//!
//! Roundhouse splits Rails' polymorphic `count` into scalar `count`
//! (Integer) and `group_count` (Hash). Class methods on the model are
//! the compiled call sites; a script against the class would miss
//! `group` (Relation-only) and hit Kernel `#select`. emit_and_run pins
//! both returns through those methods. Relation SQL — not a gem.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const METHODS: &str = r#"
  def self.title_counts
    Article.group(:title).count
  end

  def self.popular_title_counts
    Article.group(:title).having("COUNT(*) > 1").count
  end

  def self.distinct_title_count
    Article.select(:title).distinct.count
  end

  def self.distinct_row_count
    Article.distinct.count
  end

  def self.grouped_then_scalar_count
    rel = Article.group(:title)
    rel.count
  end

  def self.popular_via_alias
    Article.select("COUNT(*) AS total").group(:title).having("total > 1").count
  end

  def self.distinct_title_id_per_group
    Article.select(:title, :id).group(:title).distinct.count
  end

  def self.distinct_popular_title_counts
    Article.group(:title).distinct.having("COUNT(*) > 1").count
  end

  def self.distinct_title_named_hello
    Article.group(:title).distinct.having("articles.title = 'Hello world'").count
  end
"#;

fn overlay() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "  validates :body, presence: true, length: { minimum: 10 }\nend",
        &format!("  validates :body, presence: true, length: {{ minimum: 10 }}\n{METHODS}end"),
    )
}

fn seed_articles() -> &'static str {
    r#"
Article.create!(title: "Hello world", body: "abcdefghij")
Article.create!(title: "Hello world", body: "abcdefghij")
Article.create!(title: "Other title", body: "abcdefghij")
"#
}

#[test]
fn grouped_count_answers_a_hash_of_row_counts() {
    let script = format!(
        r#"
{seed}
got = Article.title_counts
raise "grouped hash: #{{got.inspect}}" unless got["Hello world"] == 2 && got["Other title"] == 1
raise "scalar" if got.is_a?(Integer)
puts "grouped hash passed"
"#,
        seed = seed_articles()
    );
    overlay().run_ruby(&script).assert_passes();
}

#[test]
fn grouped_count_with_having_keeps_surviving_groups() {
    let script = format!(
        r#"
{seed}
got = Article.popular_title_counts
raise "having hash: #{{got.inspect}}" unless got == {{ "Hello world" => 2 }}
plain = Article.title_counts
raise "unfiltered: #{{plain.inspect}}" unless plain.length == 2
puts "having hash passed"
"#,
        seed = seed_articles()
    );
    overlay().run_ruby(&script).assert_passes();
}

#[test]
fn select_distinct_count_uses_the_projection() {
    let script = format!(
        r#"
{seed}
titles = Article.distinct_title_count
rows = Article.distinct_row_count
all = Article.count
raise "distinct titles=#{{titles}} rows=#{{rows}} all=#{{all}}" unless titles == 2 && rows == 3 && all == 3
raise "titles not integer" unless titles.is_a?(Integer)
puts "distinct projection passed"
"#,
        seed = seed_articles()
    );
    overlay().run_ruby(&script).assert_passes();
}

#[test]
fn scalar_count_on_a_grouped_relation_counts_groups() {
    let script = format!(
        r#"
{seed}
got = Article.grouped_then_scalar_count
raise "grouped scalar: #{{got.inspect}}" unless got == 2
raise "hash" if got.is_a?(Hash)
puts "grouped scalar count passed"
"#,
        seed = seed_articles()
    );
    overlay().run_ruby(&script).assert_passes();
}

#[test]
fn grouped_count_having_on_selected_alias() {
    let script = format!(
        r#"
{seed}
got = Article.popular_via_alias
raise "alias having: #{{got.inspect}}" unless got == {{ "Hello world" => 2 }}
puts "alias having passed"
"#,
        seed = seed_articles()
    );
    overlay().run_ruby(&script).assert_passes();
}

#[test]
fn grouped_distinct_count_with_multi_column_select() {
    let script = format!(
        r#"
{seed}
got = Article.distinct_title_id_per_group
raise "grouped distinct: #{{got.inspect}}" unless got["Hello world"] == 2 && got["Other title"] == 1
raise "scalar" if got.is_a?(Integer)
puts "grouped distinct multi-column passed"
"#,
        seed = seed_articles()
    );
    overlay().run_ruby(&script).assert_passes();
}

#[test]
fn grouped_distinct_having_filters_on_original_groups() {
    let script = format!(
        r#"
{seed}
got = Article.distinct_popular_title_counts
raise "distinct having aggregate: #{{got.inspect}}" unless got == {{ "Hello world" => 2 }}
named = Article.distinct_title_named_hello
raise "distinct having column: #{{named.inspect}}" unless named == {{ "Hello world" => 2 }}
puts "distinct having before distinct passed"
"#,
        seed = seed_articles()
    );
    overlay().run_ruby(&script).assert_passes();
}
