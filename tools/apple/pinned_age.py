"""Exact official age release for private build output; no identity on CI."""
import hashlib,io,os,pathlib,platform,subprocess,tarfile
PINS={'arm64':(535541897,'e2020b073c44f692685a24d6abc378817eb81ffaaf49fd0531ef8565f767f2f5'),'x86_64':(535541887,'1d1e4bc66e1427edad7739ae7616157de0e79db8b6d2a1497d7d9925fb06a539')}
def install(directory):
    assert platform.system()=='Darwin'
    asset,digest=PINS[platform.machine()]
    raw=subprocess.check_output(['gh','api','--allow-escape-sequences','-H','Accept: application/octet-stream',f'repos/FiloSottile/age/releases/assets/{asset}'],stderr=subprocess.PIPE)
    assert hashlib.sha256(raw).hexdigest()==digest
    directory=pathlib.Path(directory);directory.mkdir(parents=True,exist_ok=False)
    with tarfile.open(fileobj=io.BytesIO(raw),mode='r:gz') as tf:
        members=[m for m in tf.getmembers() if m.name=='age/age' and m.isfile()];assert len(members)==1
        binary=directory/'age';binary.write_bytes(tf.extractfile(members[0]).read());binary.chmod(0o755)
    assert subprocess.check_output([str(binary),'--version'],text=True).strip()=='v1.3.2'
    return binary,dict(version='v1.3.2',official_asset=asset,archive_sha256=digest,binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest())
