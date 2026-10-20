# Security policy

MarkDB holds database credentials and runs SQL against real databases, so security reports are taken seriously.

## Reporting a vulnerability

Please **do not open a public issue.** Report it privately through GitHub:
[Report a vulnerability](https://github.com/thanadon-dev/markdb/security/advisories/new)

Include what an attacker can do, the steps to reproduce, and the MarkDB version. You'll get a reply within 7 days. Once a fix is released, the advisory is published with credit to you, unless you'd rather stay anonymous.

## Supported versions

Only the latest release gets security fixes. The app updates itself, so staying current is one click.

## How credentials are stored

Since v0.3.0, connection passwords are stored in Windows Credential Manager, tied to the Windows user. Connection files exported for sharing never contain passwords.
