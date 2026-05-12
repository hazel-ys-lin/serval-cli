---
api:
  path: /api/users
  method: POST
---
Feature: User creation
  Scenario: New user signs up
    When I POST /api/users
    Then status should be 201
