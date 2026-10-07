require_relative "action_controller/message_verifier"

module ActiveSupport
  module SecurityUtils
    def self.secure_compare(a, b)
      ActionController::MessageVerifier.secure_compare(a, b)
    end
  end
end
