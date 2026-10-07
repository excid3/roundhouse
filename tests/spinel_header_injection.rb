# Driver for tests/spinel_header_injection.rs — the spinel lane's HTTP
# servers never put a header line on the wire that carries a CR, LF or
# other control character.
#
# Run under plain CRuby against the real servers over a scripted socket
# (tests/tep_server_harness.rb). The app answers with values an attacker
# can steer — a redirect Location built from a param, a Content-
# Disposition built from one, a cookie option — each carrying a CRLF and
# a header of its own. Written as is, the CRLF ends the header and the
# rest becomes a header the app never set (`Set-Cookie: pwned=1`), or a
# blank line and a body of the attacker's choosing.
#
# Puma's rule, which a Rails app sits behind: a header whose key or value
# holds a control character (anything below 0x20 but tab, in a value) is
# DROPPED, and the rest of the response goes out.

require_relative "tep_server_harness"

class RecordingApp
  def dispatch(req, res)
    @bodies << req.raw_body.dup
    res.status = 302
    res.headers["X-Ok"] = "fine\tstill fine"
    res.headers["Location"] = "/next\r\nSet-Cookie: pwned=1"
    res.headers["X-Split"] = "a\nX-Injected: 1"
    res.headers["X-Bad\r\nX-Key-Injected"] = "v"
    res.headers["X-Nul"] = "a\0b"
    res.set_cookie("kept", "1", { "Path" => "/" })
    res.set_cookie("sid", "abc", { "Path" => "/\r\nX-Cookie-Injected: 1" })
    res.body = "ok"
  end
end

GET = "GET /go HTTP/1.1\r\nHost: localhost\r\n\r\n".b

SERVERS.each_key do |s|
  r = serve(s, GET)
  status, _recvs, _bodies, raised = r
  head = Sock.wire.out.split("\r\n\r\n", 2).first.to_s
  lines = head.split("\r\n")
  check("#{s}: answers the redirect", raised.nil? && status == 302, describe(*r))
  check("#{s}: no bare LF or NUL inside a header line",
        lines.none? { |l| l.include?("\n") || l.include?("\0") }, head.inspect)
  injected = lines.grep(/\A(Set-Cookie: pwned|X-Injected|X-Key-Injected|X-Cookie-Injected)/)
  check("#{s}: no injected header line", injected.empty?, injected.inspect)
  check("#{s}: the poisoned headers are dropped",
        lines.none? { |l| l.start_with?("Location:", "X-Split:", "X-Nul:", "X-Bad", "Set-Cookie: sid=") },
        lines.inspect)
  check("#{s}: the clean headers still go out (tab allowed)",
        lines.include?("X-Ok: fine\tstill fine") && lines.include?("Set-Cookie: kept=1; Path=/"),
        lines.inspect)
end

total = CHECKS.length
passed = CHECKS.count(true)
puts "#{passed}/#{total} checks pass"
puts "done"
