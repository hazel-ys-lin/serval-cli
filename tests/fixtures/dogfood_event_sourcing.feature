# Generic event-sourcing demo Gherkin matching the
# `examples/event-sourcing.toml` pattern set. Three scenarios cover
# the happy-path step types: multi-event setup, view query with
# deep-match assertion, command with emitted-event assertion, and
# an empty-payload event.

Feature: User registry view
  Show all registered users

  Rule: List registered users
    Scenario: Two users registered
      Given the UserRegistered event has occurred on stream "user-001":
        """
        {
          "name": "Alice",
          "email": "alice@example.com",
          "hashedPassword": "<<hashed>>"
        }
        """
      And the UserRegistered event has occurred on stream "user-002":
        """
        {
          "name": "Bob",
          "email": "bob@example.com",
          "hashedPassword": "<<hashed>>"
        }
        """
      When the UserList view is queried
      Then the view returns:
        """
        [
          {
            "name": "Alice",
            "userId": "user-001"
          },
          {
            "name": "Bob",
            "userId": "user-002"
          }
        ]
        """

Feature: User registration
  Register a new user account

  Rule: Successful registration
    Scenario: Anonymous registers
      Given no prior events
      When Anonymous sends RegisterUser on stream "user-001":
        """
        {
          "name": "Alice",
          "email": "alice@example.com",
          "password": "pass1234"
        }
        """
      Then the UserRegistered event is emitted with:
        """
        {
          "name": "Alice",
          "email": "alice@example.com",
          "hashedPassword": "<<hashed>>"
        }
        """

Feature: Login
  Existing user signs in

  Rule: Login with credentials
    Scenario: Anonymous logs in
      Given the UserRegistered event has occurred on stream "user-001":
        """
        {
          "name": "Alice",
          "email": "alice@example.com",
          "hashedPassword": "<<hashed>>"
        }
        """
      When Anonymous sends Login on stream "user-001":
        """
        {
          "email": "alice@example.com",
          "password": "pass1234"
        }
        """
      Then the UserLoggedIn event is emitted with:
        """
        {}
        """
