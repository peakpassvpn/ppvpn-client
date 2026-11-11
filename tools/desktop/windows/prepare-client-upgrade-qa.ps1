# Disposable CI only: ephemeral test EdDSA identity and a loopback TLS identity.
# No production signing keys, feed, account or verifier override is used.
$ErrorActionPreference='Stop'
$private=Join-Path $env:RUNNER_TEMP 'update-client-private'
New-Item -ItemType Directory -Force $private | Out-Null
$openssl=(Get-Command openssl.exe -ErrorAction SilentlyContinue).Source
if(-not $openssl){$openssl=Join-Path $env:ProgramFiles 'Git/usr/bin/openssl.exe'}
if(-not (Test-Path $openssl)){throw 'Installed OpenSSL not found'}
& $openssl genpkey -algorithm ed25519 -out (Join-Path $private 'update.key')
if($LASTEXITCODE){throw 'Test update identity generation failed'}
& $openssl pkey -in (Join-Path $private 'update.key') -pubout -outform DER -out (Join-Path $private 'update-public.der')
if($LASTEXITCODE){throw 'Test update public key extraction failed'}
$der=[IO.File]::ReadAllBytes((Join-Path $private 'update-public.der'))
$public=[Convert]::ToBase64String($der[($der.Length-32)..($der.Length-1)])
Write-Host "QA identity: EdDSA key ready; creating loopback TLS certificate"
$cert=New-SelfSignedCertificate -DnsName localhost -CertStoreLocation Cert:\CurrentUser\My -NotAfter (Get-Date).AddDays(1) -KeyExportPolicy Exportable
[IO.File]::WriteAllText((Join-Path $private 'tls-thumb.txt'),$cert.Thumbprint)
Export-Certificate -Cert $cert -FilePath (Join-Path $private 'tls.cer') | Out-Null
Write-Host "QA identity: trusting only the new loopback TLS certificate on this disposable CI machine"
Import-Certificate -FilePath (Join-Path $private 'tls.cer') -CertStoreLocation Cert:\LocalMachine\Root | Out-Null
Write-Host "QA identity: loopback TLS trust established"
$password=[Guid]::NewGuid().ToString('N')
Write-Host "::add-mask::$password"
Export-PfxCertificate -Cert $cert -FilePath (Join-Path $private 'tls.pfx') -Password (ConvertTo-SecureString $password -AsPlainText -Force) | Out-Null
$env:PPVPN_QA_PFX_PASS=$password
& $openssl pkcs12 -in (Join-Path $private 'tls.pfx') -passin env:PPVPN_QA_PFX_PASS -nocerts -nodes -out (Join-Path $private 'tls.key')
if($LASTEXITCODE){throw 'Loopback TLS test key export failed'}
& $openssl pkcs12 -in (Join-Path $private 'tls.pfx') -passin env:PPVPN_QA_PFX_PASS -clcerts -nokeys -out (Join-Path $private 'tls.crt')
if($LASTEXITCODE){throw 'Loopback TLS test certificate export failed'}
Remove-Item Env:\PPVPN_QA_PFX_PASS
$values=@{
 PPVPN_QA_PRIVATE=$private
 PPVPN_QA_TLS_THUMB=$cert.Thumbprint
 PPVPN_UPDATE_PRIVATE_KEY_FILE=(Join-Path $private 'update.key')
 PPVPN_SPARKLE_PUBLIC_KEY=$public
 PPVPN_UPDATE_PUBLIC_KEY=$public
 PPVPN_VERSION='0.7.5'
 PPVPN_UPDATE_SITE='https://localhost:18764'
 PPVPN_UPDATE_FEED_URL='https://localhost:18764/desktop/stable/appcast-windows-x64.xml'
 PPVPN_RELEASE_CHANNEL='stable'
 PPVPN_API_BASE='http://127.0.0.1:9'
 PPVPN_SKIP_ANALYZE='1'
}
$values.GetEnumerator() | ForEach-Object { Add-Content -Path $env:GITHUB_ENV -Value "$($_.Key)=$($_.Value)" -Encoding utf8 }
