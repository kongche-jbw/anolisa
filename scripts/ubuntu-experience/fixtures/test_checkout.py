"""Small real test suite with one deliberate free-shipping boundary bug."""

import pytest


def shipping_fee(subtotal):
    return 5 if subtotal <= 100 else 0


@pytest.mark.parametrize("subtotal", range(1, 81))
def test_small_order_shipping(subtotal):
    assert shipping_fee(subtotal) == 5


def test_free_shipping_at_threshold():
    assert shipping_fee(100) == 0
