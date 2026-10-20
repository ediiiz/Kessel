import { describe, expect, it, vi } from 'vitest';
import { click, render, type } from '$lib/dom-testing';
import TokenField from './TokenField.svelte';

const TOKEN = 'operator-token-value';

describe('token_is_never_rendered_outside_a_deliberate_reveal', () => {
  it('masks the token by default and reveals it only when asked', () => {
    const view = render(TokenField, {
      token: TOKEN,
      onchange: () => {
        // Not the subject of this test.
      },
    });

    const input = view.query('token-input');
    expect(input).toBeInstanceOf(HTMLInputElement);
    expect((input as HTMLInputElement).type).toBe('password');
    // Nowhere in the rendered *text* — an input's value is not text content.
    expect(view.container.textContent).not.toContain(TOKEN);

    click(view.query('token-reveal'));
    expect((view.query('token-input') as HTMLInputElement).type).toBe('text');

    click(view.query('token-reveal'));
    expect((view.query('token-input') as HTMLInputElement).type).toBe('password');
    view.destroy();
  });

  it('reports what the operator typed, and never puts it in a link or a title', () => {
    const onchange = vi.fn();
    const view = render(TokenField, { token: '', onchange });

    type(view.query('token-input'), TOKEN);

    expect(onchange).toHaveBeenCalledWith(TOKEN);
    expect(view.container.innerHTML).not.toContain(`href`);
    for (const element of view.container.querySelectorAll('[title]')) {
      expect(element.getAttribute('title')).not.toContain(TOKEN);
    }
    view.destroy();
  });
});
