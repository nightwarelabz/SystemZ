# SystemZ M0 hardware census

- Host: nightwarelabz-HP-EliteDesk-800-G1-SFF
- Date: 2026-10-30 23:50 UTC
- Kernel: Linux 7.3.0-10-generic x86_64
- Machine: HP EliteDesk 800 G1 SFF / Hewlett-Packard
- Board: 1998, BIOS L01 v02.78 (02/20/2020)

## Virtualization (the M0 gate)
- CPU: Intel(R) Core(TM) i5-4590 CPU @ 3.30GHz
- Virtualization mode: Intel VMX (vmx=yes, svm=no)
- Second-level translation: EPT=yes, NPT=no
- IOMMU groups: 0; DMAR/IOMMU in kernel log: no

## Firmware and trust
- Firmware mode: UEFI
- Secure Boot: SecureBoot disabled 
- TPM: present; /sys/class/tpm/tpm0/tpm_version_major = 1

## Memory and storage
- RAM total: 15Gi
- Storage:
  NAME   MODEL                 SIZE TRAN TYPE
  loop0                          4K      loop
  loop1                       66.8M      loop
  loop2                       14.2M      loop
  loop3                      262.3M      loop
  loop4                       12.7M      loop
  loop5                      615.3M      loop
  loop6                       91.7M      loop
  loop7                        1.5M      loop
  loop8                        402M      loop
  loop9                       18.4M      loop
  loop10                        12M      loop
  loop11                      44.7M      loop
  loop12                       828K      loop
  loop13                     222.3M      loop
  loop14                     222.9M      loop
  sda    ST1000NX0313        931.5G sata disk
  sdb    AGI500GIMAI238      476.9G sata disk
  sdc    Cruzer Glide         57.3G usb  disk
  sr0    hp DVD-ROM SN-108FB  1024M sata rom

## Network interfaces
  00:19.0 Ethernet controller: Intel Corporation Ethernet Connection I217-LM (rev 04)
  lo               UNKNOWN        00:00:00:00:00:00 <LOOPBACK,UP,LOWER_UP> 
  eno1             DOWN           8c:dc:d4:3b:75:38 <NO-CARRIER,BROADCAST,MULTICAST,UP> 
  wlx9418654a5926  UP             94:18:65:4a:59:26 <BROADCAST,MULTICAST,UP,LOWER_UP> 
  tailscale0       UNKNOWN        <POINTOPOINT,MULTICAST,NOARP,UP,LOWER_UP> 
  docker0          DOWN           86:1b:67:e6:a9:09 <NO-CARRIER,BROADCAST,MULTICAST,UP> 

## M0 verdict inputs
- VT-x/EPT or AMD-V/NPT present: yes
- IOMMU groups > 0: NO

Report written to: m0-census-nightwarelabz-HP-EliteDesk-800-G1-SFF.md
