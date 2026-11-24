import { expect, test } from 'bun:test';
import { ServiceContainer } from '../common';
import { CounterService } from './CounterService';

test('the application service retains the count until reset', () => {
  const services = new ServiceContainer();
  const counter = services.get(CounterService);
  expect(counter.increment()).toBe(1);
  expect(services.get(CounterService).increment()).toBe(2);
  counter.reset();
  expect(counter.value).toBe(0);
  services.dispose();
});
