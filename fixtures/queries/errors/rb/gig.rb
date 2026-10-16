class Gig
  def book(slot)
    raise "Slot #{slot} is taken" if slot.nil?
    raise ArgumentError, "Bad slot"
  end
end
