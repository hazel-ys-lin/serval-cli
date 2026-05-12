# language: zh-TW
# synthetic fixture exercising the shape of typical multi-Feature
# codegen exports: a `# language:` directive that does not match the
# keyword set, three Features in one file, Rule blocks with tags,
# and JSON doc strings.

Feature: Sign-up

  @happy-path @happy-path
  Rule: Email registration succeeds

    Scenario: New email registers
      Given no users exist
      When the visitor submits:
        """
        {"email": "alice@example.com", "password": "swordfish"}
        """
      Then a session token is issued

    @rate-limit
    Scenario: Rate limited after repeated attempts
      Given the visitor has tried five times
      When they try again
      Then registration is blocked

  @validation
  Rule: Duplicate email rejected

    Scenario: Duplicate email blocks sign-up
      Given a user with email "alice@example.com"
      When the visitor submits:
        """
        {"email": "alice@example.com", "password": "anything"}
        """
      Then registration fails

Feature: Login

  @happy-path
  Rule: Valid credentials authenticate

    Scenario: Login with correct password
      Given a user with email "alice@example.com" and password "swordfish"
      When credentials are submitted
      Then a session is created

Feature: Logout

  @happy-path
  Rule: Authenticated logout ends session

    Scenario: User signs out
      Given an authenticated session
      When the user logs out
      Then the session is terminated
