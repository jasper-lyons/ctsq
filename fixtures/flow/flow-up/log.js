import pino from 'pino';

const logger = pino();

export function handle(x) {
  logger.info(x);
}
