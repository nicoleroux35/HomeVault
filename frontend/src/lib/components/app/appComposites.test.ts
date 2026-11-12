/**
 * EmptyState / StatCard — the two shared page composites every screen is
 * migrating onto (UI audit W2). Component tests here are what keeps their
 * contract (labels, description, interactive 44px target) from drifting while
 * the screens change.
 */
import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import EmptyState from '$lib/components/app/EmptyState.svelte';
import StatCard from '$lib/components/app/StatCard.svelte';

describe('EmptyState', () => {
  it('renders its title, description and test hook', () => {
    render(EmptyState, {
      props: {
        title: 'No shared folders',
        description: 'Create a share to see it here.',
        'data-testid': 'empty-shares',
      },
    });

    expect(screen.getByTestId('empty-shares')).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'No shared folders' })).toBeInTheDocument();
    expect(screen.getByText('Create a share to see it here.')).toBeInTheDocument();
  });

  it('omits the description paragraph when none is given', () => {
    render(EmptyState, { props: { title: 'Nothing yet' } });
    expect(screen.getByRole('heading', { name: 'Nothing yet' })).toBeInTheDocument();
    expect(screen.queryByRole('paragraph')).not.toBeInTheDocument();
  });
});

describe('StatCard', () => {
  it('renders a non-interactive tile without a button role', () => {
    render(StatCard, { props: { label: 'Uptime', value: '12 days', sub: 'since last reboot' } });

    expect(screen.getByText('Uptime')).toBeInTheDocument();
    expect(screen.getByText('12 days')).toBeInTheDocument();
    expect(screen.getByText('since last reboot')).toBeInTheDocument();
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
  });

  it('becomes a keyboard-activatable button when onClick is supplied', async () => {
    const onClick = vi.fn();
    render(StatCard, { props: { label: 'Alerts', value: '3', onClick, 'data-testid': 'alerts' } });

    const button = screen.getByRole('button', { name: /Alerts/ });
    expect(button).toHaveAttribute('type', 'button');

    await fireEvent.click(button);
    expect(onClick).toHaveBeenCalledTimes(1);
  });
});
