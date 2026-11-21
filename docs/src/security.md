# Security

Report vulnerabilities privately through GitHub security advisories. Signed
releases use Sigstore keyless identities bound to the release workflow and an
exact stable tag (for example `v0.2.0`); pre-release tags are rejected. The
expected identity regexp is documented in the root SECURITY.md.

Supply-chain controls include cargo audit/deny, exact grammar pins, SBOM,
reproducible builds, cosign signatures and GitHub build-provenance attestations.
Meld does not claim SLSA Build L3 or any other SLSA Build level. Artifact
attestations alone are SLSA v1.0 Build Level 2 and are not a security guarantee.

See [Artifacts and verification](artifacts.md) for verification commands.

See the canonical [SECURITY.md](https://github.com/enis1enis2/meld/blob/main/SECURITY.md).
