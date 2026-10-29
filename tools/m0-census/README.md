# M0 census tool

`m0-census.sh` is read-only: it modifies nothing, installs nothing, and
touches no network. Run it on the target machine (the EliteDesk 800 G1
witness box) under any ordinary Linux:

```sh
bash m0-census.sh
```

It writes `m0-census-<hostname>.md` beside itself and prints it: CPU and
virtualization (VMX/SVM, EPT/NPT), IOMMU groups and DMAR, TPM presence,
firmware mode and Secure Boot state, RAM, storage, NICs — the exact
inputs SPEC.md milestone M0 requires, read from the machine itself.
File the output under `proofs/`.
