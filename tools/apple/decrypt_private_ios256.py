"""Authenticate the complete age payload before any archive extraction."""
import argparse,hashlib,json,os,pathlib,subprocess,tarfile

def decrypt(cipher,identity,output,expected,allow_failed=False):
    assert not output.exists();output.mkdir(mode=0o700);plaintext=output/'authenticated.tar.gz.partial'
    try:
        with cipher.open('rb') as f:assert hashlib.file_digest(f,'sha256').hexdigest()==expected['encrypted_sha256']
        result=subprocess.run(['age','--decrypt','-i',str(identity),'-o',str(plaintext),str(cipher)],capture_output=True)
        if result.returncode:raise ValueError('Complete age authentication failed')
        plaintext.chmod(0o600)
        # Successful age exit precedes tar inspection or decompression.
        with tarfile.open(plaintext) as tf:
            members=tf.getmembers();assert all(not m.issym() and not m.islnk() and not m.isdev() for m in members)
            manifest=json.load(tf.extractfile('private-build-manifest.json'))
            for key in ['source','controller','run_id','build','dispatch_nonce']:assert manifest[key]==expected[key],key
            assert manifest['policy_input_sha256']==expected['policy_sha256']
            assert manifest['domain_capability_input_sha256']==expected['domain_capability_sha256']
            if manifest['failure_class'] is None:assert manifest['policy_after_sha256']==expected['policy_sha256']
            assert manifest['sdk']=='f99761138b23f267ebc7621057afe02018ab7c18'
            assert all(manifest[k] is True for k in ['base_diagnostics','explicit_ios256_candidate','udp_target_diagnostics'])
            assert manifest['main_app_engine'] is False
            assert manifest['failure_class'] is None or allow_failed,'Private build itself failed; use explicit evidence-only mode for diagnosis'
            tf.extractall(output/'verified',filter='data')
        plaintext.rename(output/'authenticated.tar.gz')
        return manifest
    except BaseException:
        plaintext.unlink(missing_ok=True)
        raise
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('cipher',type=pathlib.Path);p.add_argument('--identity',type=pathlib.Path,required=True);p.add_argument('--metadata',type=pathlib.Path,required=True);p.add_argument('--output',type=pathlib.Path,required=True);p.add_argument('--owner-state',type=pathlib.Path,required=True);p.add_argument('--failed-build-evidence-only',action='store_true');a=p.parse_args();os.umask(0o077);state=json.loads(a.owner_state.read_text());meta=json.loads(a.metadata.read_text());expected=dict(source=state['source'],controller=state['controller'],run_id=state['run'],dispatch_nonce=state['dispatch_nonce'],build=state['diagnostic_build'],policy_sha256=state['policy_sha256'],domain_capability_sha256=state['domain_capability_sha256']);assert all(meta[k]==expected[k] for k in ['source','controller','run_id','build','dispatch_nonce']);expected['encrypted_sha256']=meta['encrypted_sha256'];manifest=decrypt(a.cipher,a.identity,a.output,expected,allow_failed=a.failed_build_evidence_only);print('Authenticated FAILED build evidence only' if manifest['failure_class'] else 'Authenticated source/run/build envelope extracted; package verification still pending')
