import { describe, expect, it } from 'vitest';
import { withinDocument } from './links';

describe('withinDocument', () => {
  it('keeps the trailing slash of the ingress directory before the fragment', () => {
    expect(withinDocument('/api/hassio_ingress/tok#/inspector')).toBe(
      '/api/hassio_ingress/tok/#/inspector',
    );
  });

  it('leaves a fragment-only href on loopback alone', () => {
    expect(withinDocument('#/config')).toBe('#/config');
  });

  it('does not double a slash that is already there', () => {
    expect(withinDocument('/api/hassio_ingress/tok/#/')).toBe('/api/hassio_ingress/tok/#/');
  });
});
