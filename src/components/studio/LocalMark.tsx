import { Microchip } from "lucide-react";
import { LOCAL_MARK } from "../../lib/words";
import { PlaceMark } from "../PlaceMark";

// [0.1.0 Studio] The mark where a model runs on this computer, in the place the Cloud mark
// takes for a cloud model: nothing of the run leaves the computer.

/** "On this computer", with what that means in its tooltip (the app's own with `tip`). */
export function LocalMark(props: { tip?: boolean }) {
  return <PlaceMark mark="local" icon={Microchip} label={LOCAL_MARK.name} title={LOCAL_MARK.title} tip={props.tip} />;
}
