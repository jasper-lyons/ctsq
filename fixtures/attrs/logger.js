import logger from 'pino'
import winston from 'winston'

export function run() {
  logger.info('pino line')
  winston.info('winston line')
  console.info('console line')
}
