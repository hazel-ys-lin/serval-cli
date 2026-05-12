Feature: Health check
  Scenario: Service is up
    When I GET /health
    Then status should be 200
