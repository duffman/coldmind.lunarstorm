import type { LogLevel, Logger } from '@coldmind/bus/src/types.ts'

export interface LoggerOptions {
  level?: LogLevel
}

const LOG_LEVELS: Record<LogLevel, number> = {
  debug: 0,
  info: 1,
  warn: 2,
  error: 3,
}

export class ConsoleLogger implements Logger {
  private readonly minLevel: number

  constructor(options: LoggerOptions = {}) {
    this.minLevel = LOG_LEVELS[options.level ?? 'debug']
  }

  error(message: string, data?: unknown): void {
    if (this.minLevel <= LOG_LEVELS.error) {
      data !== undefined ? console.error(message, data) : console.error(message)
    }
  }

  warn(message: string, data?: unknown): void {
    if (this.minLevel <= LOG_LEVELS.warn) {
      data !== undefined ? console.warn(message, data) : console.warn(message)
    }
  }

  info(message: string, data?: unknown): void {
    if (this.minLevel <= LOG_LEVELS.info) {
      data !== undefined ? console.info(message, data) : console.info(message)
    }
  }

  debug(message: string, data?: unknown): void {
    if (this.minLevel <= LOG_LEVELS.debug) {
      data !== undefined ? console.debug(message, data) : console.debug(message)
    }
  }
}

export class NoOpLogger implements Logger {
  error(): void {}
  warn(): void {}
  info(): void {}
  debug(): void {}
}

export function createDefaultLogger(_options?: LoggerOptions): Logger {
  return new NoOpLogger()
}
