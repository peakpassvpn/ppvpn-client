"""Exact integrated Mac peer and BrokerWorker checks inside the full App job."""
import hashlib,json,os,pathlib,re,subprocess,time

def execute(root,out,env):
    out=out/'macos-peer-worker-checks';out.mkdir(exist_ok=True)
    head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip()
    tree=subprocess.check_output(['git','rev-parse','HEAD^{tree}'],cwd=root,text=True).strip()
    files=['broker.rs','handshake_tests.rs','mod.rs','native.rs','worker.rs','worker_tests.rs']
    paths=['crates/service/src/single_runtime/'+name for name in files]
    paths+=['crates/service/src/single_runtime/mac_peer.rs','crates/service/src/single_runtime/image_verification.rs','crates/service/src/single_runtime/peer_timing.rs','crates/service/src/mac_identity.rs','crates/desktop/src/lib.rs','crates/desktop/src/peer_timing_context.rs','Cargo.lock']
    paths=sorted(set(paths+subprocess.check_output(['git','ls-files'],cwd=root,text=True).splitlines()))
    def manifest():return dict(head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip(),tree=subprocess.check_output(['git','rev-parse','HEAD^{tree}'],cwd=root,text=True).strip(),files={n:hashlib.sha256((root/n).read_bytes()).hexdigest() for n in paths})
    before=manifest();(out/'source-pre.json').write_text(json.dumps(before,indent=2)+'\n')
    host={}
    for name,args in [('rustc',['rustc','-vV']),('os',['sw_vers']),('arch',['uname','-m']),('xcode',['xcodebuild','-version'])]:host[name]=subprocess.check_output(args,text=True).strip()
    (out/'host-info.json').write_text(json.dumps(host,indent=2)+'\n')
    env=dict(env,PPVPN_BROKER_SOURCE_HEAD=head);results=[]
    def run(name,args):
        start=time.monotonic()
        print(json.dumps(dict(stage=name,event="begin")),flush=True)
        with (out/(name+'.stdout.raw')).open('wb') as stdout,(out/(name+'.stderr.raw')).open('wb') as stderr:
            code=subprocess.run(args,cwd=root,env=env,stdout=stdout,stderr=stderr).returncode
        item=dict(name=name,command=args,exit=code,seconds=time.monotonic()-start)
        print(json.dumps(dict(stage=name,event='end',exit=code,seconds=item['seconds'])),flush=True)
        results.append(item);(out/'results.json').write_text(json.dumps(results,indent=2)+'\n')
        if code:raise RuntimeError('Mac check failed: '+name)
        return (out/(name+'.stdout.raw')).read_text(errors='replace')
    try:
        run('desktop-peer-compile',['cargo','check','--locked','-p','ppvpn-desktop','--features','macos-user-runtime'])
        for label,selector in [('desktop-audit-and-signature','user_runtime_peer::tests::'),('desktop-image-generation','image_verification::tests::'),('desktop-peer-timing','peer_timing::tests::')]:
            text=run(label,['cargo','test','--locked','-p','ppvpn-desktop','--features','macos-user-runtime','--lib',selector,'--','--test-threads=1','--nocapture'])
            match=re.search(r'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;',text)
            assert match and int(match[1])>0 and match.groups()[1:] == ('0','0'),label
        compile_text=run('broker-libtest-compile',['cargo','test','--locked','-p','ppvpn-service','--lib','--no-run','--message-format=json'])
        artifacts=[]
        for line in compile_text.splitlines():
            try:item=json.loads(line)
            except json.JSONDecodeError:continue
            if item.get('reason')=='compiler-artifact' and item.get('target',{}).get('name')=='ppvpn_service' and item.get('profile',{}).get('test') and item.get('executable'):
                artifacts.append(pathlib.Path(item['executable']))
        assert len(artifacts)==1,artifacts
        binary=artifacts[0];binary_sha=hashlib.sha256(binary.read_bytes()).hexdigest()
        selectors=['single_runtime::handshake_tests::worker_regression_mac_maintenance_blocks_accept','single_runtime::handshake_tests::worker_regression_mac_pretransaction_blocks_identity_and_grant','single_runtime::handshake_tests::worker_regression_mac_prior_transaction_queues_second_connection']
        listing=run('broker-ignored-selector-list',[str(binary),'worker_regression_mac','--ignored','--list'])
        actual=[line.removesuffix(': test') for line in listing.splitlines() if line.endswith(': test')]
        assert sorted(actual)==sorted(selectors),(actual,selectors)
        records=[];traces=[]
        for index,selector in enumerate(selectors,1):
            label='broker-ignored-'+str(index)
            text=run(label,[str(binary),selector,'--exact','--ignored','--test-threads=1','--nocapture','--color','never'])
            match=re.search(r'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;',text)
            assert match and match.groups()==('1','0','0'),selector
            rows=[]
            for line in (out/(label+'.stderr.raw')).read_text(errors='replace').splitlines():
                try:row=json.loads(line)
                except json.JSONDecodeError:continue
                if isinstance(row,dict) and row.get('clock')=='one-fixture-Instant-origin-in-one-process':
                    assert row.get('head')==head
                    assert row.get('association')=='controlled-out-of-band-not-production-pid-inference'
                    rows.append(row)
            assert rows,'No controlled fixture trace for '+selector
            records.append(dict(selector=selector,passed=1,failed=0,ignored=0,events=len(rows)))
            traces.extend(rows)
        assert hashlib.sha256(binary.read_bytes()).hexdigest()==binary_sha
        (out/'worker-events.json').write_text(json.dumps(traces,indent=2)+'\n')
        (out/'worker3-receipt.json').write_text(json.dumps(dict(head=head,tree=tree,binary=str(binary),binary_sha256=binary_sha,selectors=records,passed=3,failed=0,ignored=0,source_association='Controlled test CID/one fixture monotonic origin; not production PID inference',scope='Actual BrokerWorker/host/protocol/HMAC/audit-token/ledger; OS signing/path controlled. Portable model, historical75f and production root-cause closure are not substituted.',coverage_boundaries=['Maintenance entry barrier does not cover every internal maintenance substep','cfg(test) CID is not production observability','No historical75f root-cause closure or new intermittent-failure claim']),indent=2)+'\n')
    finally:
        after=manifest();(out/'source-post.json').write_text(json.dumps(after,indent=2)+'\n');assert before==after,'Source drift during Mac checks'

if __name__ == '__main__':
    import sys
    assert sys.platform == 'darwin' and os.environ['SDK2A9_PLATFORM'] == 'macos'
    assert os.environ['PPVPN_INTERNAL_DIAGNOSTICS'] == os.environ['PPVPN_INTERNAL_UDP_TARGET_DIAGNOSTICS'] == '0'
    assert os.environ.get('PPVPN_INTERNAL_IOS_CAP256_CANDIDATE', '0') == '0'
    assert not os.environ.get('PPVPN_PRIVATE_UDP_POLICY_FILE') and not os.environ.get('PPVPN_PRIVATE_UDP_POLICY_SHA256')
    execute(pathlib.Path(os.environ['SDK2A9_CLIENT']),pathlib.Path(os.environ['RUNNER_TEMP'])/'sdk2a9-output',dict(os.environ))
