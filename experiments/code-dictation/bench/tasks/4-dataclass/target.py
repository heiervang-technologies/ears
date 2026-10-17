import logging
from dataclasses import dataclass

log = logging.getLogger(__name__)


@dataclass
class Order:
    id: int
    price: float
    paid: bool = False


def unpaid_total(orders):
    unpaid = [o.price for o in orders if not o.paid]
    log.info(f"{len(unpaid)} unpaid orders")
    return sum(unpaid)
