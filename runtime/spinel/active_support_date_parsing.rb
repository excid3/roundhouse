# Date-only ActiveSupport intrinsics for Spinel. Loaded only with the
# bounded Date package (`app_uses_date`) — Campfire and other date-free
# apps must not see `Date` / `Date?` in the always-on time-parsing seam
# (matz/spinel#7334).
module ActiveSupport
  # SQL DATE has no clock or zone. The empty string is the SQLite
  # adapter's nil representation for a nullable column.
  def self.parse_db_date(value)
    return nil if value.nil? || value == ""
    Date.iso8601(value)
  end

  def self.format_db_date(value)
    return nil if value.nil?
    return nil if value.is_a?(String) && value == ""
    return value.iso8601 if value.is_a?(Date)
    return Date.iso8601(value).iso8601 if value.is_a?(String)
    raise TypeError, "expected Date, String, or nil"
  end

  # Not `Date.today`: that reads the host clock, ignoring the app's zone and `travel`.
  def self.current_date
    now = ActiveSupport.now
    Date.new(now.year, now.month, now.day)
  end

  def self.date_beginning_of_month(date)
    Date.new(date.year, date.month, 1)
  end

  def self.date_end_of_month(date)
    Date.new(date.year, date.month, Date.month_length(date.year, date.month))
  end

  def self.date_beginning_of_day(date)
    ActiveSupport.local_time(date.year, date.month, date.day, 0, 0, 0, 0)
  end

  def self.date_end_of_day(date)
    ActiveSupport.local_time(date.year, date.month, date.day, 23, 59, 59, 999_999_999)
  end
end
