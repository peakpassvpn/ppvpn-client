"""Same-source internal iOS archive, encrypted before any artifact upload."""
import base64,contextlib,hashlib,json,os,pathlib,re,shutil,signal,subprocess,sys,tarfile,time
from pinned_age import install
from private_build_isolation import clone_target,child_environment

def execute():
    os.umask(0o077)
    env=dict(os.environ);temp=pathlib.Path(env['RUNNER_TEMP']);source=pathlib.Path(env['SDK2A9_CLIENT']);controller=pathlib.Path(env['GITHUB_WORKSPACE'])
    assert env['GITHUB_ACTIONS']=='true' and env['RUNNER_ENVIRONMENT']=='github-hosted'
    assert env['GITHUB_REPOSITORY']=='peakpassvpn/ppvpn-client' and env['GITHUB_EVENT_NAME']=='workflow_dispatch' and not env.get('GITHUB_HEAD_REF')
    bundle=json.loads(env['PPVPN_IOS256_PRIVATE_BUNDLE'])
    assert set(bundle)=={'schema','source','controller','dispatch_nonce','build','policy_base64','policy_sha256','recipient','expires_unix'}
    assert bundle['schema']==2 and bundle['source']==env['SOURCE_REF'] and bundle['controller']==env['GITHUB_SHA']
    assert re.fullmatch(r'[0-9a-f]{64}',bundle['dispatch_nonce']) and bundle['dispatch_nonce']==env['PPVPN_PRIVATE_DISPATCH_NONCE']
    assert env['GITHUB_RUN_ATTEMPT']=='1' and bundle['build']==int(env['PPVPN_BUILD_NUMBER'])+1
    assert time.time()<bundle['expires_unix']<time.time()+8*3600
    assert re.fullmatch(r'age1[a-z0-9]{50,200}',bundle['recipient'])
    policy=base64.b64decode(bundle['policy_base64'],validate=True)
    assert 0<len(policy)<=128 and hashlib.sha256(policy).hexdigest()==bundle['policy_sha256']
    assert re.fullmatch(r'[0-9a-f]{64}',bundle['policy_sha256'])
    assert subprocess.check_output(['git','rev-parse','HEAD'],cwd=source,text=True).strip()==bundle['source']
    assert 'f99761138b23f267ebc7621057afe02018ab7c18' in (source/'Cargo.toml').read_text()
    # Pin/validate the encryptor before materializing private build input.
    age,age_proof=install(temp/'private-age-tool')
    public=temp/'ios256-encrypted-output';public.mkdir(exist_ok=False)
    probe=public/'recipient-check.age'
    subprocess.run([str(age),'-r',bundle['recipient'],'-o',str(probe),os.devnull],check=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    probe.unlink()
    private=temp/'private-ios256';private.mkdir(mode=0o700,exist_ok=False)
    output=private/'apple-output'
    policy_path=private/'udp-policy.txt'
    result=private/'private-build-manifest.json';records=[];failure=None;archive=None
    child=child_environment(env)
    for k in ['PPVPN_IOS256_PRIVATE_BUNDLE','GH_TOKEN','GITHUB_TOKEN','IOS_TEAM_ID','IOS_APP_PROFILE','IOS_TUNNEL_PROFILE','IOS_EXPORT_OPTIONS','PPVPN_API_BASE','PPVPN_FLUTTER_HOME','PPVPN_FLUTTER_SAMPLE','PPVPN_FLUTTER_SYNTHETIC_TRAFFIC']:
        child.pop(k,None)
    child.update(CARGO_TARGET_DIR=str(private/'target'),RUNNER_TEMP=str(private),GITHUB_WORKSPACE=env['SDK2A9_GIT'],PPVPN_BUILD_NUMBER=str(bundle['build']),CANDIDATE_ONLY='1',REUSE_BASELINE_RUN='',PAIRED_CANDIDATE_JSON='',PPVPN_INTERNAL_DIAGNOSTICS='1',PPVPN_INTERNAL_IOS_CAP256_CANDIDATE='1',PPVPN_INTERNAL_UDP_TARGET_DIAGNOSTICS='1',PPVPN_PRIVATE_UDP_POLICY_FILE=str(policy_path),PPVPN_PRIVATE_UDP_POLICY_SHA256=bundle['policy_sha256'],PPVPN_SKIP_ANALYZE='1')
    def run(stage,args,cwd=source,structured=False):
        print(json.dumps(dict(stage=stage,event='begin')),flush=True);started=time.monotonic()
        with (private/(stage+'.log')).open('wb') as log, ((private/(stage+'.stderr.log')).open('wb') if structured else contextlib.nullcontext(subprocess.STDOUT)) as errors:
            process=subprocess.Popen(args,cwd=cwd,env=child,stdout=log,stderr=errors,start_new_session=True)
            active=private/'active-command.json'
            try:
                active.write_text(json.dumps(dict(pid=process.pid,run_id=env['GITHUB_RUN_ID'])))
                code=process.wait()
                # A failed command may leave children even after its leader exits.
                if code:
                    try:os.killpg(process.pid,signal.SIGKILL)
                    except ProcessLookupError:pass
            except BaseException:
                try:os.killpg(process.pid,signal.SIGTERM)
                except ProcessLookupError:pass
                try:process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    try:os.killpg(process.pid,signal.SIGKILL)
                    except ProcessLookupError:pass
                    process.wait()
                # Reap/stop any remaining descendants in this owned group.
                try:os.killpg(process.pid,signal.SIGKILL)
                except ProcessLookupError:pass
                raise
            finally:
                active.unlink(missing_ok=True)
        row=dict(stage=stage,command=args,exit=code,seconds=time.monotonic()-started);records.append(row)
        print(json.dumps(dict(stage=stage,event='end',exit=code,seconds=row['seconds'])),flush=True)
        if code:raise RuntimeError(stage)
    old_handlers={}
    def interrupted(signum,frame):raise InterruptedError('Private build interrupted')
    for signum in [signal.SIGINT,signal.SIGTERM]:old_handlers[signum]=signal.signal(signum,interrupted)
    try:
        output.mkdir()
        policy_path.write_bytes(policy);policy_path.chmod(0o600)
        # Copy-on-write seed: separate inodes, never hard links or writable aliases.
        normal_target=pathlib.Path(env['CARGO_TARGET_DIR']).resolve()
        assert normal_target==controller.resolve()/'target'
        private_target=private/'target'
        seed=clone_target(normal_target,private_target,private/'warm-seed.log')
        (private/'isolation.json').write_text(json.dumps(dict(seed=seed,private_cargo_target=str(private_target),ordinary_target_read_only=True,go_cache_variables_changed=False),indent=2)+'\n')
        config=private/'private-build.xcconfig'
        config.write_text('\n'.join([f'SYMROOT = {private}/apple-products',f'OBJROOT = {private}/apple-intermediates',f'CLANG_MODULE_CACHE_PATH = {private}/apple-module-cache',f'SWIFT_MODULE_CACHE_PATH = {private}/apple-swift-module-cache',f'SHARED_PRECOMPS_DIR = {private}/apple-precompiled','COMPILER_INDEX_STORE_ENABLE = NO','INDEX_ENABLE_DATA_STORE = NO'])+'\n');config.chmod(0o600)
        child['XCODE_XCCONFIG_FILE']=str(config)
        run('main-rust-features',['cargo','tree','--locked','--target','aarch64-apple-ios','-e','features,no-dev','-p','ppvpn-flutter'])
        run('ne-explicit-features',['cargo','tree','--locked','--target','aarch64-apple-ios','-e','features,no-dev','--prefix','none','-i','ppvpn-tunnel-host','-p','ppvpn-tunnel-host','--features','internal-ios-cap256-candidate,internal-udp-target-diagnostics'])
        graph=(private/'ne-explicit-features.log').read_text()
        for feature in ['internal-diagnostics','internal-ios-cap256-candidate','internal-udp-target-diagnostics']:assert 'feature "'+feature+'"' in graph
        run('canonical-diag-archive',['bash',str(controller/'tools/apple/archive_ios_variants.sh'),'primary'])
        candidate=output/'candidate';proof=json.loads((candidate/'build-provenance.json').read_text())
        assert proof['source_commit']==bundle['source'] and proof['build_number']==str(bundle['build']) and proof['export_contract_check_exit']==0
        project=private/'apple-candidate/apps/flutter'
        checked=[]
        for name in ['Runner','PacketTunnel']:
            stage='actual-private-settings-'+name
            run(stage,['xcodebuild','-project','ios/Runner.xcodeproj','-target',name,'-configuration','Release','-sdk','iphoneos','-showBuildSettings','-json'],cwd=project,structured=True)
            settings=json.loads((private/(stage+'.log')).read_text())
            rows=[r['buildSettings'] for r in settings if r.get('target')==name];assert len(rows)==1
            actual=rows[0];flags=actual['SWIFT_ACTIVE_COMPILATION_CONDITIONS']
            if isinstance(flags,str):flags=flags.split()
            assert all(flag in flags for flag in ['PPVPN_INTERNAL_DIAGNOSTICS','PPVPN_INTERNAL_IOS_CAP256_CANDIDATE','PPVPN_INTERNAL_UDP_TARGET_DIAGNOSTICS'])
            for key in ['SYMROOT','OBJROOT','CLANG_MODULE_CACHE_PATH','SHARED_PRECOMPS_DIR']:
                assert pathlib.Path(actual[key]).resolve().is_relative_to(private.resolve()),key
            checked.append(dict(target=name,swift_conditions=flags,private_output_settings=True))
        (private/'actual-main-ne-conditions.json').write_text(json.dumps(checked,indent=2)+'\n')
        generated=private/'apple-candidate/apps/flutter/ios/Runner'
        swift_proof=json.loads((generated/'PrivateUDPBuildPolicy.provenance.json').read_text())
        assert swift_proof['configured'] is True and swift_proof['input_sha256']==bundle['policy_sha256']
        assert hashlib.sha256(policy_path.read_bytes()).hexdigest()==bundle['policy_sha256']
        (private/'main-private-policy-provenance.json').write_text(json.dumps(swift_proof,indent=2)+'\n')
        # Contains literal-bearing Swift. Kept only inside this encrypted payload.
        shutil.copyfile(generated/'AppDelegate.swift',private/'ActualGeneratedAppDelegate.swift')
    except BaseException as error:
        failure=type(error).__name__
    finally:
        # No raw exception/input text reaches the public console or upload path.
        context=dict(schema=1,source=bundle['source'],controller=bundle['controller'],run_id=int(env['GITHUB_RUN_ID']),dispatch_nonce=bundle['dispatch_nonce'],build=bundle['build'],sdk='f99761138b23f267ebc7621057afe02018ab7c18',cargo_lock_sha256=hashlib.sha256((source/'Cargo.lock').read_bytes()).hexdigest(),base_diagnostics=True,explicit_ios256_candidate=True,udp_target_diagnostics=True,policy_input_sha256=bundle['policy_sha256'],policy_after_sha256=hashlib.sha256(policy_path.read_bytes()).hexdigest() if policy_path.exists() else None,main_app_engine=False,steps=records,failure_class=failure,age=age_proof,scope='Unsigned internal build only; no install, network workload, capacity acceptance, signing credentials or release')
        payload=private/'private-ios256-payload.tar.gz'
        try:
            result.write_text(json.dumps(context,indent=2)+'\n');result.chmod(0o600)
            # Only selected outputs/logs/manifests enter the envelope, never caches.
            with tarfile.open(payload,'w:gz') as tf:
                for p in sorted(private.iterdir()):
                    if p.name==payload.name:continue
                    if p.name in ['apple-candidate','apple-derived-candidate','udp-policy.txt','target','apple-products','apple-intermediates','apple-module-cache','apple-swift-module-cache','apple-precompiled']:continue
                    tf.add(p,arcname=p.name,recursive=True)
            payload.chmod(0o600)
            encrypted=public/'ios256-private.tar.gz.age'
            subprocess.run([str(age),'-r',bundle['recipient'],'-o',str(encrypted),str(payload)],check=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env={k:v for k,v in child.items() if not k.startswith('PPVPN_PRIVATE_UDP_')})
            with encrypted.open('rb') as f:digest=hashlib.file_digest(f,'sha256').hexdigest()
            metadata=dict(schema=1,source=bundle['source'],controller=bundle['controller'],run_id=int(env['GITHUB_RUN_ID']),dispatch_nonce=bundle['dispatch_nonce'],build=bundle['build'],encrypted_file=encrypted.name,encrypted_sha256=digest,encrypted_bytes=encrypted.stat().st_size,format='age-v1',build_succeeded=failure is None,scope='Only encrypted diagnostics; authenticate entire envelope before unpacking and verify inner source/run/build')
            (public/'encrypted-provenance.json').write_text(json.dumps(metadata,indent=2)+'\n')
        finally:
            payload.unlink(missing_ok=True)
            shutil.rmtree(private)
            (public/'plaintext-cleanup.json').write_text(json.dumps(dict(private_tree_removed=not private.exists(),payload_removed=not payload.exists(),private_target_removed=not (private/'target').exists(),ordinary_target_not_deleted=True,shared_go_cache_touched=False),indent=2)+'\n')
            for signum,handler in old_handlers.items():signal.signal(signum,handler)
    if failure:raise RuntimeError('Private build failed; encrypted evidence preserved')
    print('Private diagnostic archive encrypted; no plaintext artifact uploaded',flush=True)

if __name__=='__main__':
    try:execute()
    except BaseException as error:
        print('Private diagnostic stage did not complete: '+type(error).__name__,file=sys.stderr)
        sys.exit(1)
