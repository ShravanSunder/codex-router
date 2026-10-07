"""Deterministic ACP peer: observe real client wires; no native provider I/O."""
import json
import sys

scenario, receipt_path = sys.argv[1:]
requests = []
current = {'mode': 'agent', 'model': 'fixture-a', 'thinking': 'true',
           'context': '300k', 'effort': 'medium', 'fast': 'false'}
effort_id = 'effort'


def read_request():
    line = sys.stdin.readline()
    if not line:
        return None
    request = json.loads(line)
    requests.append(request)
    with open(receipt_path, 'w') as receipt:
        json.dump(requests, receipt)
    return request


def send(value):
    print(json.dumps(value), flush=True)


def reply(request, result):
    send({'jsonrpc': '2.0', 'id': request['id'], 'result': result})


def option(config_id, category, values, current_value):
    return {'id': config_id, 'name': config_id, 'category': category,
            'type': 'select', 'currentValue': current_value,
            'options': [{'value': value, 'name': value} for value in values]}


def options():
    models = ['fixture-a', 'fixture-b']
    efforts = ['medium', 'high']
    if scenario == 'dynamic' and current['model'] == 'fixture-a':
        efforts = ['medium']
        if current['mode'] == 'agent':
            models = ['fixture-a']
    result = [option('mode', 'mode', ['agent', 'ask'], current['mode']),
              option('model', 'model', models, current['model']),
              option('thinking', 'thought_level', ['false', 'true'], current['thinking']),
              option('context', 'model_config', ['300k', '1m'], current['context'])]
    if scenario != 'thinking-only':
        result.append(option(effort_id, 'thought_level', efforts, current['effort']))
    if scenario in ['ambiguous', 'explicit-wins', 'duplicate-explicit']:
        extra_id = 'effort' if scenario == 'duplicate-explicit' else 'reasoning'
        result.append(option(extra_id, 'thought_level', ['medium', 'high'], 'medium'))
    if scenario == 'duplicate-mixed':
        result.append({'id': 'effort', 'name': 'effort', 'category': 'thought_level',
                       'type': 'boolean', 'currentValue': True})
    result.append(option('fast', 'model_config', ['false', 'true'], current['fast']))
    return result


request = read_request()
assert request['method'] == 'initialize', request
reply(request, {'protocolVersion': 1, 'agentCapabilities': {
    'loadSession': True, 'mcpCapabilities': {'http': True}},
                'agentInfo': {'name': 'cursor-settings-wire-peer', 'version': '1'}})
if scenario == 'negotiation':
    sys.stdin.read()
    sys.exit(0)
request = read_request()
assert request['method'] in ['session/new', 'session/load'], request
if scenario in ['fallback', 'ambiguous']:
    effort_id = 'budget'
if request['method'] == 'session/new':
    reply(request, {'sessionId': 'fixture-session', 'configOptions': options()})
else:
    reply(request, {'configOptions': options()})
    effort_id = 'budget'
    send({'jsonrpc': '2.0', 'method': 'session/update', 'params': {
        'sessionId': 'fixture-session', 'update': {
            'sessionUpdate': 'config_option_update', 'configOptions': options()}}})

expected_ids = ['mode', 'model', 'effort'] if scenario == 'dynamic' else [effort_id]
for expected_id in expected_ids:
    request = read_request()
    if request is None:
        sys.exit(0)
    assert request['method'] == 'session/set_config_option', request
    if scenario in ['ambiguous', 'thinking-only', 'duplicate-explicit', 'duplicate-mixed']:
        send({'jsonrpc': '2.0', 'id': request['id'], 'error': {
            'code': -32602, 'message': 'unexpected mutation: ambiguous or absent effort'}})
        sys.stdin.read()
        sys.exit(0)
    assert request['params']['configId'] == expected_id, request
    expected_value = {'mode': 'ask', 'model': 'fixture-b'}.get(expected_id, 'high')
    assert request['params']['value'] == expected_value, request
    current['effort' if expected_id == effort_id else expected_id] = expected_value
    if scenario == 'adjusted':
        current['context'] = '1m'
        current['fast'] = 'true'
    reply(request, {'configOptions': options()})
sys.stdin.read()
