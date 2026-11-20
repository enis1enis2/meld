# Security

Report vulnerabilities privately through GitHub security advisories. Signed
releases use Sigstore keyless identities bound to the release workflow and tag;
the expected identity regexp is documented in the root SECURITY.md.

Supply-chain controls include cargo audit/deny, exact grammar pins, SBOM,
reproducible builds, cosign signatures and GitHub build-provenance attestations.
Meld does not claim SLSA Level 3.

See the canonical [SECURITY.md](https://github.com/enis1enis2/meld/blob/main/SECURITY.md).
