"""Best-effort hosted-job cleanup after success/failure/cancellation, no secret."""
import json,os,pathlib,shutil,sys

def cleanup():
    assert os.environ['GITHUB_ACTIONS']=='true' and os.environ['RUNNER_ENVIRONMENT']=='github-hosted' and sys.platform=='darwin'
    temp=pathlib.Path(os.environ['RUNNER_TEMP']).resolve();private=temp/'private-ios256';public=temp/'ios256-encrypted-output';public.mkdir(exist_ok=True)
    assert private.resolve().is_relative_to(temp)
    active=private/'active-command.json';busy=False
    if active.exists():
        row=json.loads(active.read_text());assert row['run_id']==os.environ['GITHUB_RUN_ID']
        try:os.kill(int(row['pid']),0);busy=True
        except ProcessLookupError:pass
    if busy:
        (public/'cancel-cleanup.json').write_text(json.dumps(dict(private_writer_still_active=True,cleanup_deferred_to_ephemeral_runner_teardown=True,ordinary_cache_touched=False))+'\n')
        raise RuntimeError('Private writer active; refuse cache deletion')
    if private.exists():shutil.rmtree(private)
    (public/'cancel-cleanup.json').write_text(json.dumps(dict(private_tree_removed=not private.exists(),ordinary_cache_touched=False))+'\n')
if __name__=='__main__':cleanup()
