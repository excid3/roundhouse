# frozen_string_literal: true
# Requires the runtime file directly rather than `test_helper`: this
# module needs nothing from the app tree (no DB, no schema, no
# fixtures). Run by `tests/ruby_toolchain.rs`, which overlays this
# directory onto the generated tree.
require "minitest/autorun"
require_relative "../runtime/json_column"

# Unit tests for `runtime/json_column.rb` — the schema-less JSON column
# seam, and the per-key reads and writes `lower::store_accessor` routes
# every synthesized accessor through. Sibling of schematized_json_test.
class JsonColumnTest < Minitest::Test
  def test_load_and_dump_round_trip_the_decoded_value
    assert_nil JsonColumn.load(nil)
    assert_nil JsonColumn.dump(nil)
    assert_equal({"theme" => "dark"}, JsonColumn.load("{\"theme\":\"dark\"}"))
    assert_equal "{\"theme\":\"dark\"}", JsonColumn.dump("theme" => "dark")
  end

  # ── read_key ──────────────────────────────────────────────────

  def test_read_key_answers_nil_for_an_unset_column_or_absent_key
    assert_nil JsonColumn.read_key(nil, "theme")
    assert_nil JsonColumn.read_key("{}", "theme")
    assert_nil JsonColumn.read_key("{\"other\":1}", "theme")
  end

  def test_read_key_answers_the_decoded_value_whatever_its_json_type
    assert_equal "dark", JsonColumn.read_key("{\"theme\":\"dark\"}", "theme")
    assert_equal ["a", "b"], JsonColumn.read_key("{\"features\":[\"a\",\"b\"]}", "features")
    assert_equal true, JsonColumn.read_key("{\"stripe_tax\":true}", "stripe_tax")
    assert_nil JsonColumn.read_key("{\"theme\":null}", "theme")
  end

  # ── write_key ─────────────────────────────────────────────────

  def test_write_key_starts_from_an_empty_object_when_the_column_is_unset
    assert_equal "{\"theme\":\"dark\"}", JsonColumn.write_key(nil, "theme", "dark")
    # A String is stored as a String, never coerced to a boolean.
    assert_equal "true", JsonColumn.read_key(JsonColumn.write_key(nil, "theme", "true"), "theme")
  end

  def test_write_key_merges_over_the_other_keys
    out = JsonColumn.write_key("{\"theme\":\"dark\",\"locale\":\"en\"}", "theme", "light")
    assert_equal({"theme" => "light", "locale" => "en"}, JsonColumn.load(out))
  end

  def test_write_key_stores_a_list_and_nil_as_json
    out = JsonColumn.write_key("{}", "features", ["a", "b"])
    assert_equal ["a", "b"], JsonColumn.read_key(out, "features")
    out = JsonColumn.write_key(out, "features", nil)
    assert_equal "{\"features\":null}", out
  end
end
