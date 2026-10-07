//! Relation `#count` on DISTINCT and GROUP BY queries counts the
//! result-set shape, not the underlying row total (#343).
//!
//! That is ActiveRecord::Relation SQL. A scope is the relation seed
//! (same as other overlays): `select` on a loaded Array is Enumerable.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn distinct_and_grouped_count_use_the_result_set_size() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  scope :named, -> { where.not(title: nil) }",
        )
        .run_ruby(
            r#"
%w[c c b b a].each { |t| Article.create!(title: t, body: "abcdefghij") }
# Five rows, three distinct titles. Scalar `#count` on a grouped
# relation is group cardinality (`count_sql`), not Rails' Hash of
# group → n.
n = Article.named.select("title").distinct.count
raise "distinct count #{n}" unless n == 3
g = Article.named.group("title").count
raise "grouped count #{g}" unless g == 3
sql = Article.named.select("title").distinct.count_sql
raise "distinct count_sql lost DISTINCT: #{sql}" unless sql.include?("DISTINCT")
raise "distinct count_sql lost subquery: #{sql}" unless sql.include?("__rh_count")
gsql = Article.named.group("title").count_sql
raise "grouped count_sql lost GROUP BY: #{gsql}" unless gsql.include?("GROUP BY")
raise "grouped count_sql lost subquery: #{gsql}" unless gsql.include?("__rh_count")
puts "result-set count passed"
"#,
        )
        .assert_passes();
}
