Feature: Rule walk regression

  @authentication
  Rule: Login
    Successful login paths

    Scenario: User logs in with valid credentials
      Given the user exists
      When they submit credentials
      Then they are logged in

    @rate-limit
    Scenario: Rate limited after repeated failures
      Given the user has tried five times
      When they try again
      Then they are blocked

  @management
  Rule: Logout
    Successful logout paths

    Scenario: User logs out
      Given the user is logged in
      When they click logout
      Then they are logged out
