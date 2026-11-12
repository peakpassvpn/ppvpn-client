# Reuse unchanged NORMAL installers; never re-sign, patch, overlay or launch them here.
$ErrorActionPreference='Stop'
if(-not $env:GITHUB_ACTIONS -or $env:PPVPN_QA_REUSE_CLIENT_RUN -notmatch '^\d+$'){throw 'Valid disposable CI source run required'}
$originalDir='build/reused/client-upgrade-qa'
$original=Get-Content "$originalDir/artifact-manifest.json" -Raw|ConvertFrom-Json
$source=[string]$original.source
if($source -notmatch '^[a-f0-9]{40}$' -or $original.dispatch_sha -cne $source){throw 'Original normal source identity invalid'}
$headers=@{Authorization="Bearer $env:GH_TOKEN";Accept='application/vnd.github+json'}
$run=Invoke-RestMethod -Uri "https://api.github.com/repos/peakpassvpn/ppvpn-client/actions/runs/$env:PPVPN_QA_REUSE_CLIENT_RUN" -Headers $headers
if($run.head_sha -cne $source){throw 'Original artifact source differs from its actual run'}
$jobs=(Invoke-RestMethod -Uri "https://api.github.com/repos/peakpassvpn/ppvpn-client/actions/runs/$env:PPVPN_QA_REUSE_CLIENT_RUN/jobs" -Headers $headers).jobs
$normal=@($jobs|Where-Object {$_.name -ceq 'update-notification-qa / windows-client-upgrade'})
$build=@($normal.steps|Where-Object {$_.name -ceq 'Normal A and B client and installer builds'})
if($normal.Count -ne 1 -or $build.Count -ne 1 -or $build[0].conclusion -cne 'success'){throw 'Original normal A/B build success not verified'}
git fetch --no-tags --depth=1 origin $source
if($LASTEXITCODE){throw 'Original source tree unavailable'}
$allowed=@('.github/workflows/ci.yml','.github/workflows/update-notification-qa.yml','tools/desktop/windows/client-upgrade-qa.cpp','tools/desktop/windows/run-client-upgrade-qa.ps1','tools/desktop/windows/validate-client-upgrade-reuse.ps1')
$changed=@(git diff --name-only $source HEAD --)
if($LASTEXITCODE){throw 'Unable to compare exact tracked source trees'}
foreach($path in $changed){if($path -cnotin $allowed){throw "Changed product/build input prevents normal client reuse: $path"}}
$hashes=@{}
foreach($role in @('A','B')){
  $dir="build/reused/update-installers/$role"
  $exe=Join-Path $dir 'PPVPN-0.7.5-windows-x64-setup.exe'
  $actual=(Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash
  $sidecar=(Get-Content "$exe.sha256" -Raw).Trim().Split()[0]
  $meta=Get-Content "$dir/release-meta-windows-x64.json" -Raw|ConvertFrom-Json
  $expected=if($role -eq 'A'){'0.7.5.60000'}else{'0.7.5.60001'}
  if($actual -ine $sidecar -or $actual -ine $meta.sha256 -or (Get-Item -LiteralPath $exe).Length -ne $meta.length -or $meta.build -cne $expected -or $meta.ed_signature -notmatch '^[A-Za-z0-9+/]{86}==$'){throw "Original signed $role installer/metadata mismatch"}
  $hashes[$role]=$actual
  $generated=Join-Path $originalDir "generated-runner/$role"
  if((Get-Content "$generated/source.txt" -Raw).Trim() -cne $source){throw 'Generated runner source identity mismatch'}
  foreach($line in Get-Content "$generated/sha256.txt"){
    if($line -notmatch '^([a-f0-9]{64})\s+\*?([a-z0-9_]+\.(?:cpp|h))$'){throw 'Unexpected generated source hash entry'}
    if((Get-FileHash -LiteralPath (Join-Path $generated $Matches[2]) -Algorithm SHA256).Hash -ine $Matches[1]){throw 'Generated runner source hash differs'}
  }
  $identity=ConvertFrom-StringData (Get-Content "$generated/ppvpn_window_manager/source-identity.txt" -Raw)
  if((Get-FileHash "$generated/ppvpn_window_manager/window_manager.cpp" -Algorithm SHA256).Hash -ine $identity.patched_sha256 -or (Get-FileHash "$generated/ppvpn_window_manager/window_manager_plugin.cpp" -Algorithm SHA256).Hash -ine $identity.patched_plugin_sha256){throw 'Materialized original native plugin source hash differs'}
}
$out='build/client-upgrade-qa'
New-Item -ItemType Directory -Force $out|Out-Null
Copy-Item "$originalDir/artifact-manifest.json" "$out/reused-normal-build-manifest.json"
Copy-Item "$originalDir/generated-runner" "$out/generated-runner" -Recurse
Copy-Item 'build/reused/update-installers' 'build/update-installers' -Recurse
@{product_source=$source;observer_source=(git rev-parse HEAD).Trim();original_run=$env:PPVPN_QA_REUSE_CLIENT_RUN;actual_original_run_head=$run.head_sha;normal_build_job=$normal[0].id;normal_build_conclusion=$build[0].conclusion;changed_observer_files=$changed;original_installer_sha256=$hashes;signatures='original normal A/B signatures and metadata retained unchanged; no signing or verification bypass';mode='normal installed A-to-B via existing updater, not a plugin overlay'}|ConvertTo-Json -Depth 6|Set-Content "$out/client-reuse-identity.json"
Write-Host 'Exact normal product/build inputs unchanged; original A/B package hashes and signed metadata verified. Only observers are compiled.'
