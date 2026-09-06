// Independent SDK observes loss, then explicitly reconnects; it never replays a prompt.
import net from 'node:net';
import { Readable, Writable } from 'node:stream';
import { createInterface } from 'node:readline';
import { pathToFileURL } from 'node:url';

const [sdkPath, socketPath, sessionId, cwd] = process.argv.slice(2);
if (!sdkPath || !socketPath || !sessionId || !cwd) throw new Error('Missing owned ACP recovery arguments');
const { ClientSideConnection, ndJsonStream, PROTOCOL_VERSION } = await import(pathToFileURL(sdkPath).href);
const input = createInterface({input: process.stdin, crlfDelay: Infinity});
const instructions = input[Symbol.asyncIterator]();
const carriers = [];
const emit = (value) => process.stdout.write(JSON.stringify(value) + '\n');
const timer = setTimeout(() => { for (const socket of carriers) socket.destroy(); process.exitCode = 1; input.close(); }, 140000);
let text = '';
let requestRestart = false;
let restartRequested = false;
let callbacks = 0;
async function connectClient() {
  const socket = net.createConnection(socketPath);
  carriers.push(socket);
  await new Promise((resolve, reject) => { socket.once('connect', resolve); socket.once('error', reject); });
  const closed = new Promise((resolve) => socket.once('close', () => resolve({kind: 'carrierClosed'})));
  const connection = new ClientSideConnection(() => ({
    sessionUpdate: async ({sessionId: target, update}) => {
      if (target === sessionId && update.sessionUpdate === 'agent_message_chunk' && update.content?.type === 'text') {
        text += update.content.text;
        if (text.length > 65536) throw new Error('Recovery proof output bound');
        if (requestRestart && !restartRequested) { restartRequested = true; emit({kind: 'restartRequested', sessionId}); }
      }
    },
    requestPermission: async () => { callbacks += 1; return {outcome: {outcome: 'cancelled'}}; },
  }), ndJsonStream(Writable.toWeb(socket), Readable.toWeb(socket)));
  await connection.initialize({protocolVersion: PROTOCOL_VERSION, clientCapabilities: {}});
  return {connection, socket, closed};
}
async function waitForInstruction(expected) {
  const line = await instructions.next();
  if (line.done || line.value !== expected) throw new Error('Missing explicit recovery instruction');
}
try {
  const first = await connectClient();
  await first.connection.loadSession({sessionId, cwd, mcpServers: []});
  text = ''; requestRestart = true;
  const pending = first.connection.prompt({sessionId, prompt: [{type: 'text', text: 'Do not use tools. Print integers from 1 to 10000, one number per line, until disconnected. Do not stop early.'}]})
    .then((result) => ({kind: 'promptCompleted', stopReason: result.stopReason}), (error) => ({kind: 'promptRejected', code: typeof error.code === 'number' ? error.code : null}));
  const loss = await Promise.race([pending, first.closed]);
  if (!restartRequested || loss.kind === 'promptCompleted') throw new Error('Pending ACP prompt did not fail across replacement');
  await first.closed;
  emit({kind: 'generationLost', sessionId, loss});
  await waitForInstruction('reconnect');
  requestRestart = false;
  const second = await connectClient();
  // Creation allocates a blank native thread only. No prompt or model invocation
  // is issued to this new identity; model-backed proof stays on the pinned target.
  const created = await second.connection.newSession({cwd, mcpServers: []});
  if (!created.sessionId || created.sessionId === sessionId) throw new Error('ACP new reused the existing identity');
  emit({kind: 'newSessionCreated', sessionId: created.sessionId, modelInvoked: false});
  await second.connection.loadSession({sessionId, cwd, mcpServers: []});
  emit({kind: 'reloadedSessionReady', sessionId});
  await waitForInstruction('prompt');
  text = '';
  const result = await second.connection.prompt({sessionId, prompt: [{type: 'text', text: 'Do not use tools. Reply with exactly ACP_REPLACEMENT_OK.'}]});
  if (result.stopReason !== 'end_turn' || text.trim() !== 'ACP_REPLACEMENT_OK' || callbacks !== 0) throw new Error('Explicit ACP recovery did not complete expected work');
  emit({kind: 'independentAcpRecoveryPassed', sessionId, newSessionId: created.sessionId, oldCarrierClosed: true, replayed: false, result: 'ACP_REPLACEMENT_OK'});
} finally {
  clearTimeout(timer);
  for (const socket of carriers) socket.destroy();
  input.close();
}
