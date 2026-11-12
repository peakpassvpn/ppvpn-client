"""Private build caches must never alias the ordinary build target."""
import pathlib,shutil,subprocess

def clone_target(normal_target,private_target,log_path):
    normal_target=pathlib.Path(normal_target);private_target=pathlib.Path(private_target)
    assert not private_target.exists()
    private_target.mkdir()
    if not normal_target.is_dir():return 'cold'
    with pathlib.Path(log_path).open('wb') as log:
        code=subprocess.run(['/bin/cp','-cR',str(normal_target)+'/.',str(private_target)],stdout=log,stderr=subprocess.STDOUT).returncode
    if code:
        shutil.rmtree(private_target);private_target.mkdir();return 'cold-after-clone-unavailable'
    for path in normal_target.rglob('*'):
        if path.is_file() and not path.is_symlink():
            cloned=private_target/path.relative_to(normal_target)
            assert cloned.stat().st_ino!=path.stat().st_ino,'Hard-linked seed rejected'
    for path in private_target.rglob('*'):
        if path.is_symlink():path.unlink()
    return 'APFS-copy-on-write-independent-inodes-no-symlinks'

def child_environment(env):
    child=dict(env)
    for key in list(child):
        if key in {'RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER'} or key.startswith(('SCCACHE_','CCACHE_')):
            child.pop(key,None)
    return child
