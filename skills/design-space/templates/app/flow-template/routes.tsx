import { useEffect, useState } from "react";
import { useRoute } from "../../src/shell/route";

type Order = { id: string; customer: string; total: number };

function Orders() {
  const { role, state } = useRoute();
  const [orders, setOrders] = useState<Order[] | null>(null);
  const [failed, setFailed] = useState(false);

  // The state in the toolbar decides what the mocked /api/orders answers; load again when it changes.
  useEffect(() => {
    setOrders(null);
    setFailed(false);
    fetch("/api/orders")
      .then((response) => (response.ok ? response.json() : Promise.reject(new Error(String(response.status)))))
      .then(setOrders)
      .catch(() => setFailed(true));
  }, [state]);

  if (failed) return <p role="alert">Orders could not be loaded.</p>;
  if (!orders) return <p>Loading orders...</p>;
  if (orders.length === 0) return <p>No orders yet.</p>;
  return (
    <main style={{ padding: 24, font: "16px system-ui" }}>
      <h1>Orders</h1>
      <table>
        <tbody>
          {orders.map((order) => (
            <tr key={order.id}>
              <td>{order.id}</td>
              <td>{order.customer}</td>
              <td>{order.total.toFixed(2)}</td>
              {role === "admin" && <td><button>Cancel</button></td>}
            </tr>
          ))}
        </tbody>
      </table>
    </main>
  );
}

// Each key is a page of the flow, reached at #/preview/<name><key>.
export default { "/": Orders };
