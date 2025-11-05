# Security Policy for cgx

## Reporting a Vulnerability

We take the security of `cgx` very seriously. If you believe you've found a security vulnerability, we encourage you to inform us responsibly through coordinated disclosure.

### How to Report

**Do not report security vulnerabilities through public GitHub issues, discussions, or social media.**

Instead, please use one of these secure channels:

1. **GitHub Security Advisories**: Use the "Report a vulnerability" button in the Security tab. See the [Github
   guidance on reporting security
   vulnerabilities](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability)
   for additional information.
1. **Email**: Send details to `security@cgx.sh`
1. **Backup contact**: If no response within 48 hours, email `anelson@cgx.sh`

### What to Include

To help us understand and address the issue quickly, please include:

**Required Information:**

- Brief description of the vulnerability type
- Affected version(s) and components
- Steps to reproduce the issue
- Impact assessment (what could an attacker achieve?)

**Helpful Additional Details:**

- Full paths of affected source files
- Specific commit/branch where the issue exists
- Required configuration to reproduce
- Proof-of-concept code (if available)
- Suggested mitigation or fix (if you have ideas)

### Our Response Process

**Timeline Commitments:**

- **Initial acknowledgment**: Within 48 hours
- **Detailed response**: Within 3 working days
- **Status updates**: Every 7 days until resolved
- **Resolution target**: 90 days for most issues

**What We'll Do:**

1. Acknowledge your report and assign a tracking ID
2. Assess the vulnerability and determine severity
3. Develop and test a fix
4. Coordinate disclosure timeline with you
5. Release security update and publish advisory
6. Credit you in our security advisory (if desired)

## Disclosure Policy

We follow responsible disclosure principles:

- **Coordinated disclosure**: We'll work with you to determine appropriate disclosure timing
- **Typical timeline**: 90 days from report to public disclosure
- **Early disclosure**: May occur if issue is being actively exploited
- **Delayed disclosure**: May be necessary for complex issues requiring significant changes

## Scope

This security policy applies to:

**In Scope:**

- `cgx` (all supported versions)
- Official Docker images
- Documentation that could lead to insecure configurations
- Dependencies with security implications

**Out of Scope:**

- Third-party integrations or plugins
- Issues requiring physical access to the server
- Social engineering attacks
- Attacks requiring compromised credentials (unless the vulnerability enables credential compromise)
- Theoretical vulnerabilities without practical exploitation

## Security Measures

**Our Commitments:**

- Regular security audits of dependencies using `cargo audit`
- Automated security scanning in CI/CD pipeline
- Following Rust security best practices
- Prompt security updates for critical dependencies
- Security-focused code review process

**User Responsibilities:**

- Keep `cgx` updated to the latest version (ideally by enabling auto self-update)
- Follow security configuration guidelines
- Implement proper network security (firewalls, TLS, etc.)
- Regular security monitoring and logging
- Secure credential management

## Legal Safe Harbor

We support security research conducted in good faith. If you follow these guidelines:

**We will NOT:**

- Initiate legal action against you
- Contact law enforcement about your research
- Suspend or terminate your access to `cgx` services

**You must:**

- Only test against your own `cgx` installations
- Not access, modify, or delete user data
- Not perform testing that could degrade service availability
- Not publicly disclose the issue before coordinated disclosure
- Act in good faith and not for malicious purposes

## Recognition

We believe in recognizing security researchers who help keep `cgx` secure:

- **Security Advisory Credits**: We'll credit you in our GitHub Security Advisories (unless you prefer to remain anonymous)
- **Hall of Fame**: Significant contributors may be listed in our security acknowledgments
- **Swag**: We may send `cgx` merchandise for notable contributions

## Security Updates

**Stay Informed:**

- Subscribe to our [GitHub releases](https://github.com/cgxlabs/cgx/releases) for security updates
- Join our community channels for security announcements
- Enable GitHub notifications for security advisories

**Update Process:**

- Security updates are published as patch releases (e.g., 0.12.1 → 0.12.2)
- Critical vulnerabilities may receive out-of-band releases
- Docker images are updated simultaneously with releases
- Security advisories are published through GitHub Security Advisories

## Contact Information

- **Security reports**: security@cgx.sh
- **General inquiries**: anelson@cgx.sh
- **PGP Key**: Available upon request for sensitive communications
