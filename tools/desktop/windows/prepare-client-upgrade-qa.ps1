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
# WinSparkle consumes a base64 Ed25519 seed, not OpenSSL's PKCS#8 PEM.
& $openssl pkey -in (Join-Path $private 'update.key') -outform DER -out (Join-Path $private 'update-private.der')
if($LASTEXITCODE){throw 'Test update seed extraction failed'}
$privateDer=[IO.File]::ReadAllBytes((Join-Path $private 'update-private.der'))
$prefix='302E020100300506032B657004220420'
if($privateDer.Length -ne 48 -or ([BitConverter]::ToString($privateDer[0..15]).Replace('-','')) -cne $prefix){throw 'Unexpected Ed25519 PKCS#8 structure'}
[IO.File]::WriteAllText((Join-Path $private 'update-seed.txt'),[Convert]::ToBase64String($privateDer[16..47]),(New-Object Text.UTF8Encoding($false)))
& $openssl pkey -in (Join-Path $private 'update.key') -pubout -outform DER -out (Join-Path $private 'update-public.der')
if($LASTEXITCODE){throw 'Test update public key extraction failed'}
$der=[IO.File]::ReadAllBytes((Join-Path $private 'update-public.der'))
$public=[Convert]::ToBase64String($der[($der.Length-32)..($der.Length-1)])
# Fail before either normal client build if the native signer cannot consume
# this test identity. Use exactly the signer version used by packaging.
$package=Join-Path $private 'winsparkle.zip'
Invoke-WebRequest 'https://api.nuget.org/v3-flatcontainer/winsparkle/0.9.2/winsparkle.0.9.2.nupkg' -OutFile $package
$native=Join-Path $private 'winsparkle'
Expand-Archive -LiteralPath $package -DestinationPath $native -Force
$tool=Join-Path $native 'tools/winsparkle-tool.exe'
$sample=Join-Path $private 'signer-check.txt'
[IO.File]::WriteAllText($sample,'PPVPN disposable update fixture key check')
$signature=(& $tool sign --private-key-file (Join-Path $private 'update-seed.txt') $sample | Out-String).Trim()
if($LASTEXITCODE -or $signature -notmatch '^[A-Za-z0-9+/]{86}==$'){throw 'Native test update signer rejected the ephemeral seed'}
& $tool verify --public-key $public --signature $signature $sample
if($LASTEXITCODE){throw 'Native test update signature did not verify'}
Write-Host "QA identity: native signer verified the test seed; creating loopback TLS certificate"
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
 PPVPN_UPDATE_PRIVATE_KEY_FILE=(Join-Path $private 'update-seed.txt')
 PPVPN_SPARKLE_PUBLIC_KEY=$public
 PPVPN_UPDATE_PUBLIC_KEY=$public
 PPVPN_VERSION='0.7.5'
 PPVPN_UPDATE_SITE='https://localhost:18764'
 PPVPN_UPDATE_FEED_URL='https://localhost:18764/desktop/stable/appcast-windows-x64.xml'
 PPVPN_RELEASE_CHANNEL='stable'
 PPVPN_API_BASE='https://127.0.0.1:9'
 PPVPN_API_BASE_DEFAULT='https://127.0.0.1:9'
 PPVPN_SKIP_ANALYZE='1'
}
$values.GetEnumerator() | ForEach-Object { Add-Content -Path $env:GITHUB_ENV -Value "$($_.Key)=$($_.Value)" -Encoding utf8 }
