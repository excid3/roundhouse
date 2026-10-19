# frozen_string_literal: true
# The storage boundary for schema-less ActiveRecord json/jsonb columns
# and `serialize …, coder: JSON`. SQLite hands the generated model
# serialized text; Rails exposes the decoded Array/Hash/scalar.
# Hydration writes stored text directly to the backing slot (never
# through dump); application assignment always JSON-encodes, including
# String values, so `"true"` round-trips as a string rather than bool.
require "json"

module JsonColumn
  def self.load(serialized)
    return nil if serialized.nil?
    JSON.parse(serialized)
  end

  def self.dump(value)
    return nil if value.nil?
    JSON.generate(value)
  end

  # One key of the stored object — `store_accessor`'s reader. Rails
  # treats an unset column as `{}` on the way in, so an absent column
  # and an absent key both answer nil; a stored non-object raises as
  # Rails' `Hash#[]` on an Array would.
  def self.read_key(serialized, key)
    data = load(serialized)
    return nil if data.nil?
    data[key]
  end

  # One key of the stored object assigned — `store_accessor`'s writer.
  # Returns the WHOLE re-serialized object; the caller assigns it back
  # to the column slot, the same shape as `SchematizedJson.write_*`.
  def self.write_key(serialized, key, value)
    data = load(serialized) || {}
    data[key] = value
    dump(data)
  end
end
