import { useSyncExternalStore } from "react";

import { api } from "@/lib/api";

const CACHE_SIZE = 48;
const VOLUME_KEY = "samplary-volume";
/** The volume before muting, to unmute to. */
const UNMUTED_KEY = "samplary-unmuted-volume";

interface Voice {
  source: AudioBufferSourceNode;
  gain: GainNode;
}

export interface Playing {
  id: number;
  duration: number;
}

/** Auditions one sample at a time, decoded buffers kept for quick replays. */
class Player {
  private ctx: AudioContext | null = null;
  private master: GainNode | null = null;
  private buffers = new Map<number, Promise<AudioBuffer>>();
  private voice: Voice | null = null;
  private started = 0;
  private request = 0;
  private listeners = new Set<() => void>();
  playing: Playing | null = null;
  failed: number | null = null;
  volume = Number(localStorage.getItem(VOLUME_KEY) ?? 0.8);

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  private emit() {
    for (const l of this.listeners) l();
  }

  private context() {
    if (!this.ctx) {
      this.ctx = new AudioContext();
      this.master = this.ctx.createGain();
      this.master.gain.value = this.volume;
      this.master.connect(this.ctx.destination);
    }
    return this.ctx;
  }

  /** Drops decoded buffers, for files a scan may have changed. */
  forget() {
    this.buffers.clear();
  }

  load(id: number): Promise<AudioBuffer> {
    let buffer = this.buffers.get(id);
    if (buffer) {
      this.buffers.delete(id);
    } else {
      buffer = api.pcm(id);
      buffer.catch(() => this.buffers.delete(id));
    }
    this.buffers.set(id, buffer);
    if (this.buffers.size > CACHE_SIZE) {
      this.buffers.delete(this.buffers.keys().next().value!);
    }
    return buffer;
  }

  async play(id: number, offset = 0) {
    const ctx = this.context();
    const request = ++this.request;
    void ctx.resume();
    let buffer: AudioBuffer;
    try {
      buffer = await this.load(id);
    } catch {
      if (request === this.request) {
        this.release();
        this.playing = null;
        this.failed = id;
        this.emit();
      }
      return;
    }
    if (request !== this.request) return;
    this.release();
    const gain = ctx.createGain();
    gain.connect(this.master!);
    const source = ctx.createBufferSource();
    source.buffer = buffer;
    source.connect(gain);
    const at = Math.min(Math.max(0, offset), buffer.duration);
    source.start(0, at);
    this.started = ctx.currentTime - at;
    const voice = { source, gain };
    source.onended = () => {
      if (this.voice !== voice) return;
      this.voice = null;
      this.playing = null;
      this.emit();
    };
    this.voice = voice;
    this.playing = { id, duration: buffer.duration };
    this.failed = null;
    this.emit();
  }

  stop() {
    this.request++;
    this.release();
    if (this.playing) {
      this.playing = null;
      this.emit();
    }
  }

  /** Fades the current voice out over a few milliseconds, so stops don't click. */
  private release() {
    const voice = this.voice;
    if (!voice || !this.ctx) return;
    this.voice = null;
    const now = this.ctx.currentTime;
    voice.source.onended = null;
    voice.gain.gain.setTargetAtTime(0, now, 0.004);
    voice.source.stop(now + 0.03);
    voice.source.onended = () => voice.gain.disconnect();
  }

  /** Seconds into the playing sample. */
  position(): number | null {
    if (!this.playing || !this.ctx) return null;
    return Math.min(this.ctx.currentTime - this.started, this.playing.duration);
  }

  setVolume(volume: number) {
    this.volume = volume;
    localStorage.setItem(VOLUME_KEY, String(volume));
    if (this.master) this.master.gain.value = volume;
    this.emit();
  }

  toggleMute() {
    if (this.volume > 0) {
      localStorage.setItem(UNMUTED_KEY, String(this.volume));
      this.setVolume(0);
    } else {
      this.setVolume(Number(localStorage.getItem(UNMUTED_KEY) ?? 0.8));
    }
  }
}

export const player = new Player();

export function usePlaying() {
  return useSyncExternalStore(player.subscribe, () => player.playing);
}

export function useVolume() {
  return useSyncExternalStore(player.subscribe, () => player.volume);
}

export function useFailed() {
  return useSyncExternalStore(player.subscribe, () => player.failed);
}
