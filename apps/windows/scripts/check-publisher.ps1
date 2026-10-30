# The signer of a signed file, as ppvpn-service checks its client
# (crates/service/src/ipc.rs, #489): prints the leaf certificate's subject
# and EKUs (the identity EKU, 1.3.6.1.4.1.311.97.*, for
# PPVPN_WINDOWS_PUBLISHER_EKU), and when that pin is set requires a valid
# signature, subject O = PeakPass Labs LLC and a pinned identity EKU, so a
# build never ships a service that refuses its own client.
param(
  [Parameter(Mandatory = $true)]
  [string] $Path
)

$ErrorActionPreference = "Stop"

$organization = "PeakPass Labs LLC"
$signature = Get-AuthenticodeSignature -LiteralPath $Path
$certificate = $signature.SignerCertificate
if (-not $certificate) { throw "$Path is not signed." }
$ekus = @($certificate.Extensions |
  Where-Object { $_ -is [System.Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension] } |
  ForEach-Object { $_.EnhancedKeyUsages } |
  ForEach-Object { $_.Value })
Write-Host "$(Split-Path -Leaf $Path): $($signature.Status), signed by $($certificate.Subject)"
Write-Host "  EKUs: $($ekus -join ', ')"
Write-Host "  identity EKU: $(@($ekus | Where-Object { $_ -like '1.3.6.1.4.1.311.97.*' }) -join ', ')"

$pins = @($env:PPVPN_WINDOWS_PUBLISHER_EKU, $env:PPVPN_WINDOWS_NEXT_PUBLISHER_EKU |
  ForEach-Object { "$_".Trim() } | Where-Object { $_ })
if (-not $pins) { return }
if ($signature.Status -ne "Valid") { throw "${Path}: the signature is $($signature.Status)." }
$subjectOrganization = if ($certificate.Subject -match '(?:^|,\s*)O=(?:"([^"]*)"|([^,]*))') { "$($Matches[1])$($Matches[2])" }
if ($subjectOrganization -ne $organization) {
  throw "$Path is signed by O=$subjectOrganization, not ${organization}: ppvpn-service would refuse it."
}
if (-not ($ekus | Where-Object { $pins -contains $_ })) {
  throw "$Path has none of the pinned identity EKUs ($($pins -join ', ')): ppvpn-service would refuse it."
}
Write-Host "  matches PPVPN_WINDOWS_PUBLISHER_EKU"
