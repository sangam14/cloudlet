import { renderToStaticMarkup } from 'react-dom/server';
import { expect, it } from 'vitest';
import { App } from './App';

it('renders the local console without invented VM or model activity', () => {
  const html = renderToStaticMarkup(<App />);
  expect(html).toContain('Cloudlet');
  expect(html).toContain('Main navigation');
  expect(html).toContain('Your next sandbox starts here');
  expect(html).toContain('not reported');
  expect(html).toContain('Checking connection');
  expect(html).toContain('Create sandbox');
  expect(html).not.toContain('opencode-attach');
  expect(html).not.toContain('VMM connected');
});
