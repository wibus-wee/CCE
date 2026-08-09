# Security policy

## Supported versions

Security fixes are applied to the latest minor release and the default branch.

## Reporting

Report vulnerabilities privately through GitHub Security Advisories for `wibus-wee/CCE`. Do not open a public issue before coordinated disclosure.

## Threat model

Repositories are untrusted input. CCE reads files but does not execute repository code during the default indexing pipeline. Symlinks are not followed outside the repository root, ignored/generated/binary files are excluded, file sizes are bounded, SQL uses parameters, network-backed models are opt-in, and generated summaries never become authoritative structural facts.

The default daemon binds to loopback. Exposing it beyond loopback requires an authenticating reverse proxy and explicit origin policy.

