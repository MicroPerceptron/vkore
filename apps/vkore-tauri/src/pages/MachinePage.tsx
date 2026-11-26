import { Server } from 'lucide-react';
import { Link, useParams } from 'react-router-dom';
import { useStore } from '../store';
import { Workspace } from '../tiles/Workspace';
import { Empty } from '../ui';

/** A machine is a workspace of tiles; see `tiles/`. */
export function MachinePage() {
  const { name = '' } = useParams();
  const { machines, host } = useStore();
  const machine = machines?.find((entry) => entry.name === name);
  if (!machines || !host) return <p className="muted page-pad">Reading machine…</p>;
  if (!machine) {
    return (
      <div className="card page-pad">
        <Empty icon={Server} title="No such machine">
          <Link to="/machines" className="link">Back to machines</Link>
        </Empty>
      </div>
    );
  }
  return <Workspace key={name} machine={machine} />;
}
