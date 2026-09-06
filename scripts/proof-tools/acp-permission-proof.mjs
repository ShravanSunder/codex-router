// Independent ACP permission responder. Never grants a permission or retries a prompt.
import net from 'node:net';
import { Readable, Writable } from 'node:stream';
import { pathToFileURL } from 'node:url';

const [sdkPath, socketPath, sessionId, cwd, executable] = process.argv.slice(2);
if (!sdkPath || !socketPath || !sessionId || !cwd || !executable) throw new Error('Missing owned permission proof arguments');
const { ClientSideConnection, ndJsonStream, PROTOCOL_VERSION } = await import(pathToFileURL(sdkPath).href);
const socket = net.createConnection(socketPath);
await new Promise((resolve, reject) => { socket.once('connect', resolve); socket.once('error', reject); });
let callbacks = 0;
let toolCallId;
let unexpected = false;
const connection = new ClientSideConnection(() => ({
  sessionUpdate: async (params) => { if (params.sessionId !== sessionId) unexpected = true; },
  requestPermission: async (params) => {
    callbacks += 1;
    if (params.sessionId !== sessionId || !params.toolCall?.toolCallId) unexpected = true;
    const context = (params.toolCall?.content ?? []).map((item) => item.content?.text ?? '').join('\n');
    if (params.toolCall?.kind !== 'execute' || !context.includes(executable)) unexpected = true;
    toolCallId = params.toolCall?.toolCallId;
    return {outcome: {outcome: 'cancelled'}};
  },
}), ndJsonStream(Writable.toWeb(socket), Readable.toWeb(socket)));
const timer = setTimeout(() => socket.destroy(new Error('Permission proof deadline')), 100000);
try {
  await connection.initialize({protocolVersion: PROTOCOL_VERSION, clientCapabilities: {}});
  await connection.loadSession({sessionId, cwd, mcpServers: []});
  let promptOutcome;
  try {
    const response = await connection.prompt({sessionId, prompt: [{type: 'text', text:
      `Run exactly this executable once using the shell tool: ${JSON.stringify(executable)}. It is an owned test fixture. Do not read or edit files, do not run other commands, and do not retry if permission is denied or cancelled. Stop after the command outcome.`}]});
    promptOutcome = {stopReason: response.stopReason};
  } catch (error) {
    if (error.code !== -32603) throw error;
    promptOutcome = {errorCode: error.code};
  }
  if (callbacks !== 1 || unexpected || !toolCallId) throw new Error('Expected exactly one permission request for the owned thread');
  process.stdout.write(JSON.stringify({kind: 'independentAcpPermissionCancelled', sessionId, toolCallId, callbacks, promptOutcome}) + '\n');
} finally {
  clearTimeout(timer);
  socket.destroy();
}
