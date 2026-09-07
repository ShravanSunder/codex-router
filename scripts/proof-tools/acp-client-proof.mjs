// Independent pinned ACP SDK against the owned debug listener; no model selection fallback.
import net from 'node:net';
import { spawn } from 'node:child_process';
import { dirname } from 'node:path';
import { Readable, Writable } from 'node:stream';
import { pathToFileURL } from 'node:url';

const [sdkPath, socketPath, sessionId, cwd] = process.argv.slice(2);
if (!sdkPath || !socketPath || !sessionId || !cwd) throw new Error('Expected SDK, socket, owned Luna session and cwd');
const { ClientSideConnection, ndJsonStream, PROTOCOL_VERSION } = await import(pathToFileURL(sdkPath).href);
const bridgeBinary = process.env.CODEX_ROUTER_ACP_PROOF_BRIDGE;
let socket;
let bridge;
let bridgeExit;
let input;
let output;
if (bridgeBinary) {
  bridge = spawn(bridgeBinary, ['acp', '--endpoint', 'codex-local', '--service-directory', dirname(socketPath)], {stdio:['pipe','pipe','ignore']});
  bridgeExit = new Promise((resolve) => bridge.once('exit', (code, signal) => resolve({code, signal})));
  await new Promise((resolve, reject) => { bridge.once('spawn',resolve);bridge.once('error',reject); });
  input = bridge.stdin; output = bridge.stdout;
} else {
  socket = net.createConnection(socketPath);
  await new Promise((resolve, reject) => { socket.once('connect',resolve);socket.once('error',reject); });
  input = socket; output = socket;
}
const closeCarrier = () => {
  input.destroy(); output.destroy();
  if (bridge && bridge.exitCode === null && bridge.signalCode === null) bridge.kill('SIGTERM');
};
const terminate = () => { closeCarrier(); process.exitCode = 1; };
process.once('SIGTERM', terminate);
let text = '';
let updates = 0;
let permissionRequests = 0;
let cancelOnOutput = false;
let cancelSent = false;
const connection = new ClientSideConnection(() => ({
  sessionUpdate: async (params) => {
    if (params.sessionId !== sessionId) throw new Error('Unexpected session update');
    updates += 1;
    const update = params.update;
    if (update.sessionUpdate === 'agent_message_chunk' && update.content?.type === 'text') {
      text += update.content.text;
      if (text.length > 65536) throw new Error('ACP proof output budget exceeded');
      if (cancelOnOutput && !cancelSent) {
        cancelSent = true;
        await connection.cancel({sessionId});
      }
    }
  },
  requestPermission: async () => { permissionRequests += 1; return { outcome:{ outcome:'cancelled' } }; },
}), ndJsonStream(Writable.toWeb(input), Readable.toWeb(output)));
const timeout = setTimeout(terminate, 120000);
try {
  const initialized = await connection.initialize({protocolVersion:PROTOCOL_VERSION,clientCapabilities:{},clientInfo:{name:'independent-debug-proof',version:'1'}});
  if (initialized.protocolVersion !== 1 || !initialized.agentCapabilities?.loadSession) throw new Error('Missing ACP baseline/load support');
  await connection.loadSession({sessionId,cwd,mcpServers:[]});
  text = ''; updates = 0;
  const response = await connection.prompt({sessionId,prompt:[{type:'text',text:'Do not use tools or modify files. Reply with exactly ACP_LUNA_OK.'}]});
  if (response.stopReason !== 'end_turn' || text.trim() !== 'ACP_LUNA_OK' || updates === 0 || permissionRequests !== 0) throw new Error('ACP prompt proof did not match expected outcome');
  const firstUpdates = updates;
  text = ''; updates = 0; cancelOnOutput = true;
  const cancelled = await connection.prompt({sessionId,prompt:[{type:'text',text:'Do not use tools. Produce a numbered list from 1 to 5000, one number per line, until cancelled.'}]});
  if (!cancelSent || cancelled.stopReason !== 'cancelled') throw new Error('ACP cancellation was not observed');
  process.stdout.write(JSON.stringify({kind:'independentAcpPromptPassed',carrier:bridgeBinary?'cliBridge':'unixSocket',sessionId,stopReason:response.stopReason,updates:firstUpdates,cancellation:{sent:cancelSent,stopReason:cancelled.stopReason}})+'\n');
} finally {
  clearTimeout(timeout);
  closeCarrier();
  if (bridgeExit) {
    let cleanupTimer;
    try {
      await Promise.race([bridgeExit, new Promise((_, reject) => {
        cleanupTimer = setTimeout(() => { bridge.kill('SIGKILL'); reject(new Error('Owned ACP bridge cleanup timed out')); }, 3000);
      })]);
    } finally { clearTimeout(cleanupTimer); }
  }
  process.removeListener('SIGTERM', terminate);
}
