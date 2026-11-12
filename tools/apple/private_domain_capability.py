"""Validate Main-only build intent; this never authorizes runtime traffic."""
import hashlib,ipaddress,json,re

def validate_domain_capability(raw,policy,policy_sha):
    assert len(raw)<=4096 and hashlib.sha256(policy).hexdigest()==policy_sha
    value=json.loads(raw)
    expected={'schema','kind','campaign_contract','package','protocol','ordinary_enabled','public_authority','runtime_target_manifest','same_private_policy_sha256','exact_domain','port','target_ipv4'}
    assert set(value)==expected
    assert type(value['schema']) is int and value['schema']==1
    assert value['kind']=='CAPACITY_V3_MAIN_DOMAIN_BUILD_INPUT'
    assert value['campaign_contract']=='CAPACITY_V3_TCP64_8K_UDP_CANCEL'
    assert value['package']=='com.peakpassvpn.ppvpn' and value['protocol']=='udp'
    assert value['ordinary_enabled'] is False and value['public_authority']=='UNBOUND_BUILD_INPUT_ONLY'
    assert isinstance(value['runtime_target_manifest'],str) and value['runtime_target_manifest']
    host,port=policy.decode('ascii').strip().split(':')
    assert str(ipaddress.IPv4Address(host))==host and value['target_ipv4']==host
    assert type(value['port']) is int and value['port']==int(port) and 1<=value['port']<=65535
    assert value['same_private_policy_sha256']==policy_sha
    domain=value['exact_domain'];assert isinstance(domain,str) and domain==domain.lower() and len(domain)<=253
    assert len(domain.split('.'))>=2 and all(re.fullmatch(r'[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?',part) for part in domain.split('.'))
    try:ipaddress.ip_address(domain)
    except ValueError:pass
    else:raise AssertionError('Domain capability requires one exact name')
    return value
