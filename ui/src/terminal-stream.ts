/** Ordered, acknowledged terminal frames. A lost input acknowledgement is never
 * replayed: reconnect resumes output only, from the last rendered byte. */
import type { TerminalResize, TerminalSize } from './terminal-geometry.ts'
export interface TerminalOutput { data_base64: string; seq: number; reset: boolean; geometry?: TerminalSize; resizes?: TerminalResize[] }
export type TerminalSocket = Pick<WebSocket, 'readyState' | 'bufferedAmount' | 'onopen' | 'onmessage' | 'onerror' | 'onclose' | 'send' | 'close'>;
export interface StreamOptions {
  url: string; token: string; sessionId: string; after: number; native?: boolean;
  onOutput: (output: TerminalOutput) => Promise<void>;
  onDisconnect: (error: Error) => void; onEnd: () => void;
  socketFactory?: (url: string) => TerminalSocket;
}
export class TerminalStream {
  readonly ready: Promise<void>;
  private socket: TerminalSocket;
  private readyResolve!: () => void;
  private readyReject!: (error: Error) => void;
  private connected = false;
  private ended = false;
  private disposed = false;
  private nextId = 1;
  private sequence: number;
  private processingOutput = false;
  private readyTimer: ReturnType<typeof setTimeout>;
  private heartbeat: ReturnType<typeof setInterval>;
  private lastMessage = Date.now();
  private pending = new Map<number, { resolve: () => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> }>();

  private options: StreamOptions;
  constructor(options: StreamOptions) {
    this.options = options;
    this.sequence = options.after;
    this.ready = new Promise((resolve, reject) => { this.readyResolve = resolve; this.readyReject = reject });
    this.socket = (options.socketFactory || (url => new WebSocket(url)))(options.url);
    this.readyTimer = setTimeout(() => this.fail(new Error('Не удалось открыть поток терминала.')), 5000);
    this.heartbeat = setInterval(() => {
      if (!this.connected || this.disposed) return;
      if (Date.now() - this.lastMessage > 30000) this.fail(new Error('Соединение терминала прервано. Неподтверждённый ввод не повторяется.'));
      else { try { this.send({ type: 'ping' }) } catch { this.fail(new Error('Поток терминала не готов. Ввод не отправлен.')) } }
    }, 10000);
    this.socket.onopen = () => {
      if (this.disposed) return;
      try { this.send({ type: 'auth', token: options.token, ...(options.native ? { session_id: options.sessionId } : {}), after: options.after }) } catch { this.fail(new Error('Не удалось открыть поток терминала.')) }
    };
    this.socket.onmessage = event => { void this.receive(event.data) };
    this.socket.onerror = () => this.fail(new Error('Соединение терминала прервано. Неподтверждённый ввод не повторяется.'));
    this.socket.onclose = () => { if (!this.disposed) this.fail(new Error('Соединение терминала прервано. Неподтверждённый ввод не повторяется.')) };
  }
  private send(value: unknown) {
    if (this.disposed || this.socket.readyState !== 1 || this.socket.bufferedAmount > 64 * 1024) throw new Error('Поток терминала не готов. Ввод не отправлен.');
    this.socket.send(JSON.stringify(value));
  }
  private async receive(raw: unknown) {
    if (this.disposed) return;
    try {
      if (typeof raw !== 'string' || raw.length > 128 * 1024) throw new Error('Некорректный поток терминала.');
      const frame = JSON.parse(raw);
      this.lastMessage = Date.now();
      if (frame.type === 'ready' && !this.connected) {
        this.connected = true; clearTimeout(this.readyTimer); this.readyResolve(); return;
      }
      if (!this.connected) throw new Error('Не удалось открыть поток терминала.');
      if (frame.type === 'output') {
        if (this.processingOutput || !Number.isSafeInteger(frame.seq) || frame.seq < 0 || typeof frame.reset !== 'boolean' || typeof frame.data_base64 !== 'string' || frame.data_base64.length > 96 * 1024 || !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(frame.data_base64)) throw new Error('Некорректный поток терминала.');
        const bytes = atob(frame.data_base64).length;
        if (frame.seq < bytes || (!frame.reset && frame.seq - this.sequence !== bytes)) throw new Error('Некорректный поток терминала.');
        this.processingOutput = true;
        await this.options.onOutput(frame);
        if (this.disposed) return;
        this.sequence = frame.seq;
        this.processingOutput = false;
        this.send({ type: 'ack', seq: frame.seq });
      } else if (frame.type === 'ack' && Number.isSafeInteger(frame.id)) {
        const operation = this.pending.get(frame.id);
        if (operation) { clearTimeout(operation.timer); this.pending.delete(frame.id); operation.resolve() }
      } else if (frame.type === 'error') {
        const failure = new Error(typeof frame.message === 'string' && frame.message.length <= 500 ? frame.message : 'Некорректный поток терминала.');
        const operation = this.pending.get(frame.id);
        if (operation) { clearTimeout(operation.timer); this.pending.delete(frame.id); operation.reject(failure) }
        else throw failure;
      } else if (frame.type === 'end') {
        this.ended = true; this.close(); this.options.onEnd();
      } else if (frame.type !== 'pong') throw new Error('Некорректный поток терминала.');
    } catch (error) { this.fail(error instanceof Error ? error : new Error('Некорректный поток терминала.')) }
  }
  private async command(value: Record<string, unknown>) {
    await this.ready;
    if (this.disposed || this.pending.size >= 32) throw new Error('Поток терминала не готов. Ввод не отправлен.');
    const id = this.nextId++;
    return new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => this.fail(new Error('Соединение терминала прервано. Неподтверждённый ввод не повторяется.')), 5000);
      this.pending.set(id, { resolve, reject, timer });
      try { this.send({ ...value, id }) }
      catch (error) { this.fail(error instanceof Error ? error : new Error('Поток терминала не готов. Ввод не отправлен.')) }
    });
  }
  async input(data: string) {
    if (new TextEncoder().encode(data).byteLength > 16 * 1024) return Promise.reject(new Error('Ввод не отправлен: вставьте не более 16 KiB за один раз.'));
    for (const chunk of terminalInputChunks(data)) await this.command({ type: 'input', data: chunk });
  }
  resize(cols: number, rows: number) { return this.command({ type: 'resize', cols, rows }) }
  private fail(error: Error) {
    if (this.disposed) return;
    this.close(error);
    if (!this.ended) this.options.onDisconnect(error);
  }
  close(error = new Error('Поток терминала закрыт.')) {
    if (this.disposed) return;
    this.disposed = true; clearTimeout(this.readyTimer); clearInterval(this.heartbeat);
    this.readyReject(error);
    for (const operation of this.pending.values()) { clearTimeout(operation.timer); operation.reject(error) }
    this.pending.clear();
    this.socket.onopen = this.socket.onmessage = this.socket.onerror = this.socket.onclose = null;
    this.socket.close();
  }
}

/** JSON escaping can expand 16KiB of control bytes to 96KiB. Keep each frame
 * below the protocol limit without splitting UTF8 characters or adding bytes. */
export function terminalInputChunks(data: string): string[] {
  const chunks: string[] = []; let part = '', bytes = 0;
  const encoder = new TextEncoder();
  for (const character of data) {
    const cost = encoder.encode(JSON.stringify(character)).byteLength - 2;
    if (bytes + cost > 16 * 1024 && part) { chunks.push(part); part = ''; bytes = 0 }
    part += character; bytes += cost;
  }
  if (part) chunks.push(part);
  return chunks;
}

/** Reconnect must wait for queued xterm writes before taking its resume cursor.
 * A slow/background WebView may render after the socket has already closed. */
export class TerminalRenderQueue {
  sequence = 0;
  private pending = Promise.resolve();
  write(output: TerminalOutput, consume: (output: TerminalOutput) => Promise<void>) {
    this.pending = this.pending.then(() => consume(output)).then(() => { this.sequence = output.seq });
    return this.pending;
  }
  async cursor() { await this.pending; return this.sequence }
}

/** Coalesce short bursts without local echo, preserving every byte and paste
 * boundary. Only one acknowledged batch is in flight; no automatic retries. */
export class TerminalInputBatcher {
  private queue: { data: string; bytes: number; resolve: () => void; reject: (error: Error) => void }[] = [];
  private inFlight: typeof this.queue = [];
  private bytes = 0;
  private sending = false;
  private stopped = false;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private send: (data: string) => Promise<void>;
  constructor(send: (data: string) => Promise<void>) { this.send = send }
  enqueue(data: string): Promise<void> {
    const bytes = new TextEncoder().encode(data).byteLength;
    if (this.stopped) return Promise.reject(new Error('Поток терминала закрыт.'));
    if (bytes > 16 * 1024) return Promise.reject(new Error('Ввод не отправлен: вставьте не более 16 KiB за один раз.'));
    if (this.bytes + bytes > 64 * 1024) return Promise.reject(new Error('Очередь ввода заполнена (64 KiB). Последний ввод не отправлен; проверьте текст в терминале перед продолжением.'));
    this.bytes += bytes;
    const promise = new Promise<void>((resolve, reject) => { this.queue.push({ data, bytes, resolve, reject }) });
    this.schedule(); return promise;
  }
  private schedule() { if (!this.stopped && !this.sending && !this.timer && this.queue.length) this.timer = setTimeout(() => { this.timer = undefined; void this.flush() }, 8) }
  private async flush() {
    if (this.stopped || this.sending || !this.queue.length) return;
    const batch = []; let size = 0;
    while (this.queue.length && size + this.queue[0].bytes <= 16 * 1024) { const item = this.queue.shift()!; batch.push(item); size += item.bytes }
    this.sending = true;
    this.inFlight = batch;
    try { await this.send(batch.map(item => item.data).join('')); batch.forEach(item => item.resolve()) }
    catch (error) { const failure = error instanceof Error ? error : new Error('Поток терминала закрыт.'); batch.forEach(item => item.reject(failure)); this.cancel(failure) }
    finally { this.bytes -= size; this.inFlight = []; this.sending = false; this.schedule() }
  }
  cancel(error: Error) { clearTimeout(this.timer); this.timer = undefined; for (const item of this.queue) { this.bytes -= item.bytes; item.reject(error) } this.queue = [] }
  dispose() { this.stopped = true; const error = new Error('Поток терминала закрыт.'); this.cancel(error); this.inFlight.forEach(item => item.reject(error)) }
}
